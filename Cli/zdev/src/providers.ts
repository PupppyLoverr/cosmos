// Providers per CONTRACT.md — openai-compatible / anthropic / google / fixture.
// Every HTTP call goes through the §12 host allowlist. Streaming via SSE.
import { newId } from "./proto";
import * as proto2 from "./proto";
import type { Message } from "./store";

export type Chunk = Record<string, any>; // {"type": "text"|"tool_use"|"done"|"err", ...}
export type Sink = (c: Chunk) => void;

export function hostAllowed(url: string, allowed: string[]): void {
  const host = url
    .split("://")
    .pop()!
    .split("/")[0]
    .split(":")[0]
    .split("?")[0]
    .toLowerCase();
  const ok = allowed.some(
    (a) => host === a.toLowerCase() || host.endsWith("." + a.toLowerCase()),
  );
  if (!ok) {
    throw new Error(`privacy: outbound host ${host} not on the allowlist`);
  }
}

function apiKey(cfg: Record<string, any>): string | null {
  const env = cfg.api_key_env ?? "";
  if (!env) return null;
  return process.env[env] ?? null;
}

// Read an SSE response body, invoking f(data) per `data:` payload.
// Returns early when f returns true or `[DONE]` arrives.
async function sse(
  resp: Response,
  f: (data: string) => boolean | void,
): Promise<void> {
  const reader = resp.body!.getReader();
  const dec = new TextDecoder();
  let buf = "";
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
  try {
    reader.cancel();
  } catch {}
}

async function postJson(
  url: string,
  body: Record<string, any>,
  headers: Record<string, string>,
): Promise<Response> {
  let resp: Response;
  try {
    resp = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...headers },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(300_000),
    });
  } catch (e: any) {
    throw new Error(`provider network error: ${e?.message ?? e}`);
  }
  if (!resp.ok) {
    const text = (await resp.text().catch(() => "")).slice(0, 400);
    throw new Error(`provider ${resp.status}: ${text}`);
  }
  return resp;
}

function openaiMessages(req: Record<string, any>): Record<string, any>[] {
  const out: Record<string, any>[] = req.system.map((s: string) => ({
    role: "system",
    content: s,
  }));
  for (const m of req.messages as Message[]) {
    if (m.role === "user") {
      for (const b of m.content) {
        if (b.type === "text") {
          out.push({ role: "user", content: b.text });
        } else if (b.type === "tool_result") {
          out.push({
            role: "tool",
            tool_call_id: b.tool_use_id,
            content: b.content,
          });
        }
      }
    } else if (m.role === "assistant") {
      const text = m.content
        .filter((b) => b.type === "text")
        .map((b) => b.text)
        .join("\n");
      const calls = m.content
        .filter((b) => b.type === "tool_use")
        .map((b) => ({
          id: b.id,
          type: "function",
          function: { name: b.name, arguments: JSON.stringify(b.input) },
        }));
      const msg: Record<string, any> = { role: "assistant" };
      if (text) msg.content = text;
      if (calls.length) msg.tool_calls = calls;
      out.push(msg);
    }
  }
  return out;
}

async function openai(
  cfg: Record<string, any>,
  req: Record<string, any>,
  sink: Sink,
  allowed: string[],
): Promise<void> {
  const url = cfg.base_url.replace(/\/+$/, "") + "/chat/completions";
  hostAllowed(url, allowed);
  const tools = (req.tools as any[]).map((t) => ({
    type: "function",
    function: {
      name: t.name,
      description: t.description,
      parameters: t.input_schema,
    },
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
  const headers: Record<string, string> = {};
  const k = apiKey(cfg);
  if (k) headers.authorization = `Bearer ${k}`;
  const resp = await postJson(url, body, headers);
  const acc = new Map<number, { id: string; name: string; args: string }>();
  let usageSeen = false;

  const resp2 = resp;
  await sse(resp2, (data) => {
    let v: any;
    try {
      v = JSON.parse(data);
    } catch {
      return;
    }
    const u = v.usage;
    if (u) {
      usageSeen = true;
      sink({
        type: "done",
        usage: {
          input:
            (u.prompt_tokens ?? 0) -
            (u.prompt_tokens_details?.cached_tokens ?? 0),
          cache_read: u.prompt_tokens_details?.cached_tokens ?? 0,
          cache_write: 0,
          output: u.completion_tokens ?? 0,
        },
      });
    }
    for (const ch of v.choices ?? []) {
      const d = ch.delta ?? {};
      if (d.content) sink({ type: "text", text: d.content });
      for (const tc of d.tool_calls ?? []) {
        const idx = tc.index ?? 0;
        let slot = acc.get(idx);
        if (!slot) {
          slot = { id: "", name: "", args: "" };
          acc.set(idx, slot);
        }
        if (tc.id) slot.id = tc.id;
        const fn = tc.function ?? {};
        if (fn.name) slot.name += fn.name;
        if (fn.arguments) slot.args += fn.arguments;
      }
    }
  });
  for (const slot of acc.values()) {
    if (slot.id) {
      let inp: any;
      try {
        inp = JSON.parse(slot.args || "{}");
      } catch {
        inp = {};
      }
      sink({ type: "tool_use", id: slot.id, name: slot.name, input: inp });
    }
  }
  if (!usageSeen) {
    sink({
      type: "done",
      usage: { input: 0, cache_read: 0, cache_write: 0, output: 0 },
    });
  }
}

async function anthropic(
  cfg: Record<string, any>,
  req: Record<string, any>,
  sink: Sink,
  allowed: string[],
): Promise<void> {
  const url = cfg.base_url.replace(/\/+$/, "") + "/v1/messages";
  hostAllowed(url, allowed);
  const sysBlocks: Record<string, any>[] = (req.system as string[]).map(
    (s) => ({ type: "text", text: s }),
  );
  if (sysBlocks.length)
    sysBlocks[sysBlocks.length - 1].cache_control = { type: "ephemeral" };
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
      if (b.type === "text") {
        parts.push({ type: "text", text: b.text });
      } else if (b.type === "tool_use") {
        parts.push({
          type: "tool_use",
          id: b.id,
          name: b.name,
          input: b.input,
        });
      } else if (b.type === "tool_result") {
        parts.push({
          type: "tool_result",
          tool_use_id: b.tool_use_id,
          content: [{ type: "text", text: b.content }],
          is_error: b.is_error ?? false,
        });
      }
    }
    if (parts.length) {
      msgs.push({
        role: m.role === "user" ? "user" : "assistant",
        content: parts,
      });
    }
  }
  const body = {
    model: cfg.model,
    system: sysBlocks,
    messages: msgs,
    tools,
    max_tokens: req.max_tokens,
    stream: true,
  };
  const headers: Record<string, string> = {
    "anthropic-version": "2023-06-01",
  };
  const k = apiKey(cfg);
  if (k) headers["x-api-key"] = k;
  const resp = await postJson(url, body, headers);
  const acc = new Map<number, { id: string; name: string; args: string }>();

  await sse(resp, (data) => {
    let v: any;
    try {
      v = JSON.parse(data);
    } catch {
      return;
    }
    const ty = v.type ?? "";
    if (
      ty === "content_block_start" &&
      v.content_block?.type === "tool_use"
    ) {
      const i = v.index ?? 0;
      acc.set(i, {
        id: v.content_block.id ?? "",
        name: v.content_block.name ?? "",
        args: "",
      });
    } else if (ty === "content_block_delta") {
      const i = v.index ?? 0;
      const d = v.delta ?? {};
      if (d.type === "text_delta" && d.text) {
        sink({ type: "text", text: d.text });
      } else if (d.type === "input_json_delta" && acc.has(i)) {
        acc.get(i)!.args += d.partial_json ?? "";
      }
    } else if (ty === "message_start" || ty === "message_delta") {
      const u = v.message?.usage ?? v.usage ?? {};
      sink({
        type: "done",
        usage: {
          input: u.input_tokens ?? 0,
          cache_read: u.cache_read_input_tokens ?? 0,
          cache_write: u.cache_creation_input_tokens ?? 0,
          output: u.output_tokens ?? 0,
        },
      });
    }
  });
  for (const slot of acc.values()) {
    if (slot.name) {
      let inp: any;
      try {
        inp = JSON.parse(slot.args || "{}");
      } catch {
        inp = {};
      }
      sink({ type: "tool_use", id: slot.id, name: slot.name, input: inp });
    }
  }
}

async function google(
  cfg: Record<string, any>,
  req: Record<string, any>,
  sink: Sink,
  allowed: string[],
): Promise<void> {
  const url =
    cfg.base_url.replace(/\/+$/, "") +
    `/v1beta/models/${cfg.model}:streamGenerateContent?alt=sse`;
  hostAllowed(url, allowed);
  const contents: Record<string, any>[] = [];
  for (const m of req.messages as Message[]) {
    const parts: Record<string, any>[] = [];
    for (const b of m.content) {
      if (b.type === "text") {
        parts.push({ text: b.text });
      } else if (b.type === "tool_use") {
        parts.push({ functionCall: { name: b.name, args: b.input } });
      } else if (b.type === "tool_result") {
        parts.push({
          functionResponse: { name: "tool", response: { output: b.content } },
        });
      }
    }
    if (parts.length) {
      contents.push({
        role: m.role === "assistant" ? "model" : "user",
        parts,
      });
    }
  }
  const body = {
    systemInstruction: {
      parts: (req.system as string[]).map((s) => ({ text: s })),
    },
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
  };
  const headers: Record<string, string> = {};
  const k = apiKey(cfg);
  if (k) headers["x-goog-api-key"] = k;
  const resp = await postJson(url, body, headers);

  await sse(resp, (data) => {
    let v: any;
    try {
      v = JSON.parse(data);
    } catch {
      return;
    }
    for (const cand of v.candidates ?? []) {
      for (const p of cand.content?.parts ?? []) {
        if (p.text) sink({ type: "text", text: p.text });
        if (p.functionCall) {
          sink({
            type: "tool_use",
            id: newId(),
            name: p.functionCall.name ?? "",
            input: p.functionCall.args ?? {},
          });
        }
      }
    }
    const u = v.usageMetadata;
    if (u) {
      sink({
        type: "done",
        usage: {
          input: (u.promptTokenCount ?? 0) - (u.cachedContentTokenCount ?? 0),
          cache_read: u.cachedContentTokenCount ?? 0,
          cache_write: 0,
          output: u.candidatesTokenCount ?? 0,
        },
      });
    }
  });
}

function fixture(
  cfg: Record<string, any>,
  req: Record<string, any>,
  sink: Sink,
): void {
  // Deterministic offline provider. `ping`→`pong`; `tool:NAME {json}`→call.
  let last = "";
  let hasToolResult = false;
  if (req.messages.length) {
    const m = req.messages[req.messages.length - 1];
    const texts = m.content
      .filter((b: any) => b.type === "text")
      .map((b: any) => b.text);
    last = texts.join(" ");
    hasToolResult = m.content.some((b: any) => b.type === "tool_result");
  }
  const est = (s: string) => Math.floor(s.length / 4);
  const sysT = (req.system as string[]).reduce((a, s) => a + est(s), 0);
  const msgT = (req.messages as Message[]).reduce(
    (a, m) =>
      a +
      m.content.reduce(
        (b2, b) => b2 + (b.type === "text" ? est(b.text) : 4),
        0,
      ),
    0,
  );
  sink({
    type: "done",
    usage: {
      input: sysT + msgT,
      cache_read: 0,
      cache_write: 0,
      output: 0,
    },
  });
  if (hasToolResult) {
    sink({ type: "text", text: "done." });
  } else if (last.startsWith("tool:")) {
    const rest = last.slice(5);
    const sp = rest.indexOf(" ");
    const name = sp < 0 ? rest : rest.slice(0, sp);
    const js = sp < 0 ? "" : rest.slice(sp + 1);
    let inp: any;
    try {
      inp = js ? JSON.parse(js) : {};
    } catch {
      inp = {};
    }
    sink({ type: "tool_use", id: newId(), name, input: inp });
  } else if (last.toLowerCase().includes("ping")) {
    sink({ type: "text", text: "pong" });
  } else {
    sink({
      type: "text",
      text:
        "fixture: no live provider configured — received " +
        `${JSON.stringify(last.slice(0, 80))}. Configure [providers] + [models] in config.toml for a real model.`,
    });
  }
  sink({
    type: "done",
    usage: {
      input: 0,
      cache_read: 0,
      cache_write: 0,
      output: Math.floor(JSON.stringify(cfg).length / 4) / 2 + 8,
    },
  });
}

/** ChatGPT subscription — Codex's backend Responses API. Auth comes from
 * ~/.cosmos-cli/openai-auth.json (PKCE login), not an env key. */
async function chatgpt(
  cfg: Record<string, any>,
  req: Record<string, any>,
  sink: Sink,
  allowed: string[],
): Promise<void> {
  const { accessToken } = await import("./oauth");
  const auth = await accessToken(cfg.home ?? proto2.cliHome());
  const url = cfg.base_url.replace(/\/+$/, "") + "/responses";
  hostAllowed(url, allowed);

  // transcript → Responses `input` items
  const input: Record<string, any>[] = [];
  for (const m of req.messages as Message[]) {
    for (const b of m.content) {
      if (b.type === "text") {
        input.push({
          type: "message",
          role: m.role === "assistant" ? "assistant" : "user",
          content: [
            {
              type: m.role === "assistant" ? "output_text" : "input_text",
              text: b.text,
            },
          ],
        });
      } else if (b.type === "tool_use") {
        input.push({
          type: "function_call",
          call_id: b.id,
          name: b.name,
          arguments: JSON.stringify(b.input ?? {}),
        });
      } else if (b.type === "tool_result") {
        input.push({
          type: "function_call_output",
          call_id: b.tool_use_id,
          output: typeof b.content === "string" ? b.content : JSON.stringify(b.content),
        });
      }
    }
  }
  const tools = (req.tools as any[]).map((t) => ({
    type: "function",
    name: t.name,
    description: t.description ?? "",
    parameters: t.input_schema,
    strict: false,
  }));
  const body: Record<string, any> = {
    model: cfg.model,
    instructions: (req.system as string[]).join("\n\n"),
    input,
    tools,
    tool_choice: "auto",
    parallel_tool_calls: false,
    stream: true,
    store: false,
  };
  const headers: Record<string, string> = {
    authorization: `Bearer ${auth.access_token}`,
    "OpenAI-Beta": "responses=experimental",
    originator: "zdev",
    accept: "text/event-stream",
  };
  if (auth.account_id) headers["chatgpt-account-id"] = auth.account_id;

  let resp = await postJson(url, body, headers).catch(async (e) => {
    // one refresh retry on 401 — token may have expired mid-session
    if (String(e?.message ?? "").includes("401")) {
      const fresh = await accessToken(cfg.home ?? proto2.cliHome());
      headers.authorization = `Bearer ${fresh.access_token}`;
      return postJson(url, body, headers);
    }
    throw e;
  });

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
        let inp: any;
        try {
          inp = JSON.parse(it.arguments || "{}");
        } catch {
          inp = {};
        }
        sink({
          type: "tool_use",
          id: it.call_id ?? it.id ?? newId(),
          name: it.name,
          input: inp,
        });
      }
    } else if (ty === "response.completed" || ty === "response.done") {
      const u = (v.response ?? v).usage ?? {};
      if (u.input_tokens != null || u.output_tokens != null) {
        usageSeen = true;
        sink({
          type: "done",
          usage: {
            input:
              (u.input_tokens ?? 0) -
              (u.input_tokens_details?.cached_tokens ?? 0),
            cache_read: u.input_tokens_details?.cached_tokens ?? 0,
            cache_write: 0,
            output: u.output_tokens ?? 0,
          },
        });
      }
    } else if (ty === "response.failed" || ty === "error") {
      const msg =
        v.response?.error?.message ?? v.error?.message ?? "provider error";
      sink({ type: "err", text: msg });
    }
  });
  if (!usageSeen) {
    sink({
      type: "done",
      usage: { input: 0, cache_read: 0, cache_write: 0, output: 0 },
    });
  }
}

export async function complete(
  cfg: Record<string, any>,
  req: Record<string, any>,
  sink: Sink,
  allowed: string[],
): Promise<void> {
  const kind = cfg.kind ?? "fixture";
  if (kind === "fixture") return fixture(cfg, req, sink);
  if (kind === "openai") return openai(cfg, req, sink, allowed);
  if (kind === "anthropic") return anthropic(cfg, req, sink, allowed);
  if (kind === "google") return google(cfg, req, sink, allowed);
  if (kind === "chatgpt") return chatgpt(cfg, req, sink, allowed);
  throw new Error(`unknown provider kind: ${kind}`);
}
