// Providers per CONTRACT.md — OpenAI chat completions, OpenAI Responses,
// the ChatGPT subscription (Responses on chatgpt.com), Anthropic Messages,
// Gemini and the offline fixture. Every HTTP call goes through the §12 host
// allowlist; every stream is SSE and can be aborted mid-flight.
import { newId } from "./proto";
import type { Message } from "./store";

export type Chunk = Record<string, any>; // {"type": "text"|"tool_use"|"done"|"err", ...}
export type Sink = (c: Chunk) => void;

export function hostAllowed(url: string, allowed: string[]): void {
  const host = url.split("://").pop()!.split("/")[0].split(":")[0].split("?")[0].toLowerCase();
  const ok = allowed.some((a) => host === a.toLowerCase() || host.endsWith("." + a.toLowerCase()));
  if (!ok) throw new Error(`privacy: outbound host ${host} not on the allowlist`);
}

function apiKey(cfg: Record<string, any>): string | null {
  if (cfg.api_key) return cfg.api_key;
  const env = cfg.api_key_env ?? "";
  return env ? (process.env[env] ?? null) : null;
}

/** ChatGPT plan usage from the last subscription response (for /usage). */
export type RateLimits = {
  plan?: string;
  primaryUsed?: number;
  primaryResetSecs?: number;
  secondaryUsed?: number;
  secondaryResetSecs?: number;
};
let rateLimits: RateLimits | null = null;
export const lastRateLimits = () => rateLimits;

/** Readable provider errors, with a hint for the usual causes. */
export function httpError(status: number, body: string): Error {
  let msg = body.slice(0, 400);
  try {
    const v = JSON.parse(body);
    msg = v?.error?.message ?? v?.detail ?? v?.message ?? (typeof v?.error === "string" ? v.error : msg);
  } catch {}
  const hint =
    status === 401 || status === 403
      ? " — check the API key (/connect, or zdev login for ChatGPT)"
      : status === 404
        ? " — unknown model or endpoint for this provider"
        : status === 429
          ? " — rate limited or out of quota"
          : "";
  return new Error(`provider ${status}: ${String(msg).trim()}${hint}`);
}

// Read an SSE response body, invoking f(data) per `data:` payload.
async function sse(resp: Response, f: (data: string) => boolean | void): Promise<void> {
  const reader = resp.body!.getReader();
  const dec = new TextDecoder();
  let buf = "";
  try {
    outer: while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let i: number;
      while ((i = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, i).replace(/\r$/, "");
        buf = buf.slice(i + 1);
        if (!line.startsWith("data:")) continue;
        const data = line.slice(5).trim();
        if (data === "[DONE]") break outer;
        if (f(data)) break outer;
      }
    }
  } finally {
    try {
      reader.cancel();
    } catch {}
  }
}

async function postJson(
  url: string,
  body: Record<string, any>,
  headers: Record<string, string>,
  signal?: AbortSignal,
): Promise<Response> {
  let resp: Response;
  try {
    resp = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...headers },
      body: JSON.stringify(body),
      signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(600_000)]) : AbortSignal.timeout(600_000),
    });
  } catch (e: any) {
    if (signal?.aborted) throw new Error("interrupted");
    throw new Error(`provider network error: ${e?.message ?? e} (${url.split("/").slice(0, 3).join("/")})`);
  }
  if (!resp.ok) {
    const text = await resp.text().catch(() => "");
    throw httpError(resp.status, text);
  }
  return resp;
}

const ZERO = { input: 0, cache_read: 0, cache_write: 0, output: 0 };

// ── OpenAI chat completions ─────────────────────────────────────────────────

function openaiMessages(req: Record<string, any>): Record<string, any>[] {
  const out: Record<string, any>[] = req.system.map((s: string) => ({ role: "system", content: s }));
  for (const m of req.messages as Message[]) {
    if (m.role === "user") {
      for (const b of m.content) {
        if (b.type === "text") out.push({ role: "user", content: b.text });
        else if (b.type === "tool_result") out.push({ role: "tool", tool_call_id: b.tool_use_id, content: b.content });
      }
    } else if (m.role === "assistant") {
      const text = m.content.filter((b) => b.type === "text").map((b) => b.text).join("\n");
      const calls = m.content
        .filter((b) => b.type === "tool_use")
        .map((b) => ({ id: b.id, type: "function", function: { name: b.name, arguments: JSON.stringify(b.input) } }));
      const msg: Record<string, any> = { role: "assistant", content: text || null };
      if (calls.length) msg.tool_calls = calls;
      out.push(msg);
    }
  }
  return out;
}

async function openai(cfg: Record<string, any>, req: Record<string, any>, sink: Sink, allowed: string[], signal?: AbortSignal) {
  const url = cfg.base_url.replace(/\/+$/, "") + "/chat/completions";
  hostAllowed(url, allowed);
  const tools = (req.tools as any[]).map((t) => ({
    type: "function",
    function: { name: t.name, description: t.description, parameters: t.input_schema },
  }));
  const body: Record<string, any> = {
    model: cfg.model,
    messages: openaiMessages(req),
    max_tokens: req.max_tokens,
    stream: true,
    stream_options: { include_usage: true },
    prompt_cache_key: req.session_id,
  };
  if (tools.length) body.tools = tools;
  if (cfg.effort) body.reasoning_effort = cfg.effort;
  const headers: Record<string, string> = {};
  const k = apiKey(cfg);
  if (k) headers.authorization = `Bearer ${k}`;
  const resp = await postJson(url, body, headers, signal);
  const acc = new Map<number, { id: string; name: string; args: string }>();
  let usageSeen = false;
  await sse(resp, (data) => {
    let v: any;
    try {
      v = JSON.parse(data);
    } catch {
      return;
    }
    if (v.error) sink({ type: "err", text: v.error.message ?? JSON.stringify(v.error) });
    const u = v.usage;
    if (u) {
      usageSeen = true;
      const cached = u.prompt_tokens_details?.cached_tokens ?? 0;
      sink({
        type: "done",
        usage: { input: (u.prompt_tokens ?? 0) - cached, cache_read: cached, cache_write: 0, output: u.completion_tokens ?? 0 },
      });
    }
    for (const ch of v.choices ?? []) {
      const d = ch.delta ?? {};
      if (d.content) sink({ type: "text", text: d.content });
      for (const tc of d.tool_calls ?? []) {
        const idx = tc.index ?? 0;
        let slot = acc.get(idx);
        if (!slot) acc.set(idx, (slot = { id: "", name: "", args: "" }));
        if (tc.id) slot.id = tc.id;
        const fn = tc.function ?? {};
        if (fn.name) slot.name += fn.name;
        if (fn.arguments) slot.args += fn.arguments;
      }
    }
  });
  for (const slot of acc.values()) {
    if (!slot.name) continue;
    let inp: any = {};
    try {
      inp = JSON.parse(slot.args || "{}");
    } catch {}
    sink({ type: "tool_use", id: slot.id || newId(), name: slot.name, input: inp });
  }
  if (!usageSeen) sink({ type: "done", usage: ZERO });
}

// ── OpenAI Responses (API keys) and the ChatGPT subscription ───────────────

function responsesInput(req: Record<string, any>): Record<string, any>[] {
  const out: Record<string, any>[] = [];
  for (const m of req.messages as Message[]) {
    for (const b of m.content) {
      if (b.type === "text") {
        const asst = m.role === "assistant";
        out.push({
          type: "message",
          role: asst ? "assistant" : "user",
          content: [{ type: asst ? "output_text" : "input_text", text: b.text }],
        });
      } else if (b.type === "tool_use") {
        out.push({ type: "function_call", call_id: b.id, name: b.name, arguments: JSON.stringify(b.input ?? {}) });
      } else if (b.type === "tool_result") {
        out.push({
          type: "function_call_output",
          call_id: b.tool_use_id,
          output: typeof b.content === "string" ? b.content : JSON.stringify(b.content),
        });
      }
    }
  }
  return out;
}

async function responses(cfg: Record<string, any>, req: Record<string, any>, sink: Sink, allowed: string[], signal?: AbortSignal) {
  const chatgpt = cfg.kind === "chatgpt";
  const base = cfg.base_url.replace(/\/+$/, "");
  // catalog base is https://chatgpt.com/backend-api; older configs point at …/codex
  const url = chatgpt ? (base.endsWith("/codex") ? base : base + "/codex") + "/responses" : base + "/responses";
  hostAllowed(url, allowed);
  const body: Record<string, any> = {
    model: cfg.model,
    instructions: (req.system as string[]).join("\n\n"),
    input: responsesInput(req),
    tools: (req.tools as any[]).map((t) => ({
      type: "function",
      name: t.name,
      description: t.description ?? "",
      parameters: t.input_schema,
    })),
    tool_choice: "auto",
    parallel_tool_calls: true,
    stream: true,
    store: false,
    prompt_cache_key: req.session_id,
  };
  if (!chatgpt) body.max_output_tokens = Math.max(req.max_tokens, 4096);
  if (cfg.effort) body.reasoning = { effort: cfg.effort, summary: "auto" };
  const headers: Record<string, string> = { accept: "text/event-stream" };
  let account: string | undefined;
  if (chatgpt) {
    const { fresh } = await import("./oauth");
    const t = await fresh();
    if (!t) throw new Error("not signed in to ChatGPT — run /login (or zdev login)");
    headers.authorization = `Bearer ${t.access}`;
    headers["OpenAI-Beta"] = "responses=experimental";
    headers.originator = "zdev";
    headers.session_id = req.session_id;
    account = t.account_id;
    if (account) headers["chatgpt-account-id"] = account;
  } else {
    const k = apiKey(cfg);
    if (k) headers.authorization = `Bearer ${k}`;
  }
  const resp = await postJson(url, body, headers, signal);
  if (chatgpt) {
    const f = (k: string) => {
      const v = parseFloat(resp.headers.get(k) ?? "");
      return Number.isFinite(v) ? v : undefined;
    };
    const rl: RateLimits = {
      plan: resp.headers.get("x-codex-plan-type") ?? undefined,
      primaryUsed: f("x-codex-primary-used-percent"),
      primaryResetSecs: f("x-codex-primary-reset-after-seconds"),
      secondaryUsed: f("x-codex-secondary-used-percent"),
      secondaryResetSecs: f("x-codex-secondary-reset-after-seconds"),
    };
    if (rl.plan || rl.primaryUsed !== undefined || rl.secondaryUsed !== undefined) rateLimits = rl;
  }
  let usageSeen = false;
  await sse(resp, (data) => {
    let v: any;
    try {
      v = JSON.parse(data);
    } catch {
      return;
    }
    const ty = v.type ?? "";
    if (ty === "response.output_text.delta" && v.delta) {
      sink({ type: "text", text: v.delta });
    } else if (ty === "response.output_item.done") {
      const it = v.item ?? {};
      if (it.type === "function_call" && it.name) {
        let inp: any = {};
        try {
          inp = JSON.parse(it.arguments || "{}");
        } catch {}
        sink({ type: "tool_use", id: it.call_id ?? it.id ?? newId(), name: it.name, input: inp });
      }
    } else if (ty === "response.completed" || ty === "response.done") {
      const u = (v.response ?? v).usage ?? {};
      if (u.input_tokens != null || u.output_tokens != null) {
        usageSeen = true;
        const cached = u.input_tokens_details?.cached_tokens ?? 0;
        sink({
          type: "done",
          usage: { input: (u.input_tokens ?? 0) - cached, cache_read: cached, cache_write: 0, output: u.output_tokens ?? 0 },
        });
      }
    } else if (ty === "response.failed" || ty === "error") {
      sink({ type: "err", text: v.response?.error?.message ?? v.error?.message ?? v.message ?? "provider error" });
    }
  });
  if (!usageSeen) sink({ type: "done", usage: ZERO });
}

// ── Anthropic Messages ──────────────────────────────────────────────────────

async function anthropic(cfg: Record<string, any>, req: Record<string, any>, sink: Sink, allowed: string[], signal?: AbortSignal) {
  const url = cfg.base_url.replace(/\/+$/, "") + "/v1/messages";
  hostAllowed(url, allowed);
  const sysBlocks: Record<string, any>[] = (req.system as string[]).map((s) => ({ type: "text", text: s }));
  if (sysBlocks.length) sysBlocks[sysBlocks.length - 1].cache_control = { type: "ephemeral" };
  const tools: Record<string, any>[] = (req.tools as any[]).map((t) => ({
    name: t.name,
    description: t.description,
    input_schema: t.input_schema,
  }));
  if (tools.length) tools[tools.length - 1].cache_control = { type: "ephemeral" };
  const msgs: Record<string, any>[] = [];
  for (const m of req.messages as Message[]) {
    const parts: Record<string, any>[] = [];
    for (const b of m.content) {
      if (b.type === "text") parts.push({ type: "text", text: b.text });
      else if (b.type === "tool_use") parts.push({ type: "tool_use", id: b.id, name: b.name, input: b.input });
      else if (b.type === "tool_result")
        parts.push({
          type: "tool_result",
          tool_use_id: b.tool_use_id,
          content: [{ type: "text", text: b.content || "(no output)" }],
          is_error: b.is_error ?? false,
        });
    }
    if (!parts.length) continue;
    const role = m.role === "user" ? "user" : "assistant";
    const last = msgs[msgs.length - 1];
    // consecutive same-role turns (several tool results) merge into one message
    if (last && last.role === role) last.content.push(...parts);
    else msgs.push({ role, content: parts });
  }
  // cache the conversation prefix too
  const lastMsg = msgs[msgs.length - 1];
  if (lastMsg) lastMsg.content[lastMsg.content.length - 1].cache_control = { type: "ephemeral" };
  const body = { model: cfg.model, system: sysBlocks, messages: msgs, tools, max_tokens: req.max_tokens, stream: true };
  const headers: Record<string, string> = { "anthropic-version": "2023-06-01" };
  const k = apiKey(cfg);
  if (k) headers["x-api-key"] = k;
  const resp = await postJson(url, body, headers, signal);
  const acc = new Map<number, { id: string; name: string; args: string }>();
  const usage = { ...ZERO };
  await sse(resp, (data) => {
    let v: any;
    try {
      v = JSON.parse(data);
    } catch {
      return;
    }
    const ty = v.type ?? "";
    if (ty === "content_block_start" && v.content_block?.type === "tool_use") {
      acc.set(v.index ?? 0, { id: v.content_block.id ?? "", name: v.content_block.name ?? "", args: "" });
    } else if (ty === "content_block_delta") {
      const d = v.delta ?? {};
      if (d.type === "text_delta" && d.text) sink({ type: "text", text: d.text });
      else if (d.type === "input_json_delta" && acc.has(v.index ?? 0)) acc.get(v.index ?? 0)!.args += d.partial_json ?? "";
    } else if (ty === "message_start" || ty === "message_delta") {
      const u = v.message?.usage ?? v.usage ?? {};
      // message_start carries input + cache; message_delta the running output
      if (u.input_tokens != null) usage.input = u.input_tokens;
      if (u.cache_read_input_tokens != null) usage.cache_read = u.cache_read_input_tokens;
      if (u.cache_creation_input_tokens != null) usage.cache_write = u.cache_creation_input_tokens;
      if (u.output_tokens != null) usage.output = u.output_tokens;
    } else if (ty === "error") {
      sink({ type: "err", text: v.error?.message ?? "provider error" });
    }
  });
  sink({ type: "done", usage });
  for (const slot of acc.values()) {
    if (!slot.name) continue;
    let inp: any = {};
    try {
      inp = JSON.parse(slot.args || "{}");
    } catch {}
    sink({ type: "tool_use", id: slot.id, name: slot.name, input: inp });
  }
}

// ── Gemini ──────────────────────────────────────────────────────────────────

async function google(cfg: Record<string, any>, req: Record<string, any>, sink: Sink, allowed: string[], signal?: AbortSignal) {
  const url = cfg.base_url.replace(/\/+$/, "") + `/v1beta/models/${cfg.model}:streamGenerateContent?alt=sse`;
  hostAllowed(url, allowed);
  // functionResponse must carry the function's name — map call ids back
  const names = new Map<string, string>();
  const contents: Record<string, any>[] = [];
  for (const m of req.messages as Message[]) {
    const parts: Record<string, any>[] = [];
    for (const b of m.content) {
      if (b.type === "text") parts.push({ text: b.text });
      else if (b.type === "tool_use") {
        names.set(b.id, b.name);
        parts.push({ functionCall: { name: b.name, args: b.input } });
      } else if (b.type === "tool_result") {
        parts.push({ functionResponse: { name: names.get(b.tool_use_id) ?? "tool", response: { output: b.content } } });
      }
    }
    if (!parts.length) continue;
    const role = m.role === "assistant" ? "model" : "user";
    const last = contents[contents.length - 1];
    if (last && last.role === role) last.parts.push(...parts);
    else contents.push({ role, parts });
  }
  const body = {
    systemInstruction: { parts: (req.system as string[]).map((s) => ({ text: s })) },
    contents,
    tools: [
      {
        functionDeclarations: (req.tools as any[]).map((t) => ({
          name: t.name,
          description: t.description,
          parameters: t.input_schema,
        })),
      },
    ],
    generationConfig: { maxOutputTokens: req.max_tokens },
  };
  const headers: Record<string, string> = {};
  const k = apiKey(cfg);
  if (k) headers["x-goog-api-key"] = k;
  const resp = await postJson(url, body, headers, signal);
  let usage = { ...ZERO };
  await sse(resp, (data) => {
    let v: any;
    try {
      v = JSON.parse(data);
    } catch {
      return;
    }
    for (const cand of v.candidates ?? []) {
      for (const p of cand.content?.parts ?? []) {
        if (p.text && !p.thought) sink({ type: "text", text: p.text });
        if (p.functionCall) sink({ type: "tool_use", id: newId(), name: p.functionCall.name ?? "", input: p.functionCall.args ?? {} });
      }
    }
    const u = v.usageMetadata;
    if (u) {
      const cached = u.cachedContentTokenCount ?? 0;
      usage = { input: (u.promptTokenCount ?? 0) - cached, cache_read: cached, cache_write: 0, output: u.candidatesTokenCount ?? 0 };
    }
  });
  sink({ type: "done", usage });
}

// ── fixture ─────────────────────────────────────────────────────────────────

function fixture(cfg: Record<string, any>, req: Record<string, any>, sink: Sink): void {
  // Deterministic offline provider. `ping`→`pong`; `tool:NAME {json}`→call.
  let last = "";
  let hasToolResult = false;
  if (req.messages.length) {
    const m = req.messages[req.messages.length - 1];
    last = m.content
      .filter((b: any) => b.type === "text")
      .map((b: any) => b.text)
      .join(" ");
    hasToolResult = m.content.some((b: any) => b.type === "tool_result");
  }
  const est = (s: string) => Math.floor(s.length / 4);
  const sysT = (req.system as string[]).reduce((a, s) => a + est(s), 0);
  const msgT = (req.messages as Message[]).reduce(
    (a, m) => a + m.content.reduce((b2, b) => b2 + (b.type === "text" ? est(b.text) : 4), 0),
    0,
  );
  sink({ type: "done", usage: { input: sysT + msgT, cache_read: 0, cache_write: 0, output: 0 } });
  if (hasToolResult) {
    sink({ type: "text", text: "done." });
  } else if (last.startsWith("tool:")) {
    const rest = last.slice(5);
    const sp = rest.indexOf(" ");
    const name = sp < 0 ? rest : rest.slice(0, sp);
    const js = sp < 0 ? "" : rest.slice(sp + 1);
    let inp: any = {};
    try {
      inp = js ? JSON.parse(js) : {};
    } catch {}
    sink({ type: "tool_use", id: newId(), name, input: inp });
  } else if (last.toLowerCase().includes("ping")) {
    sink({ type: "text", text: "pong" });
  } else {
    sink({
      type: "text",
      text:
        "fixture: no live provider configured — received " +
        `${JSON.stringify(last.slice(0, 80))}. Connect one with /connect or /login, or pick a model with /model.`,
    });
  }
  sink({
    type: "done",
    usage: { input: 0, cache_read: 0, cache_write: 0, output: Math.floor(JSON.stringify(cfg).length / 4) / 2 + 8 },
  });
}

export async function complete(
  cfg: Record<string, any>,
  req: Record<string, any>,
  sink: Sink,
  allowed: string[],
  signal?: AbortSignal,
): Promise<void> {
  const kind = cfg.kind ?? "fixture";
  if (kind === "fixture") return fixture(cfg, req, sink);
  if (kind === "openai") return openai(cfg, req, sink, allowed, signal);
  if (kind === "responses" || kind === "openai-responses" || kind === "chatgpt") return responses(cfg, req, sink, allowed, signal);
  if (kind === "anthropic") return anthropic(cfg, req, sink, allowed, signal);
  if (kind === "google") return google(cfg, req, sink, allowed, signal);
  throw new Error(`unknown provider kind: ${kind}`);
}
