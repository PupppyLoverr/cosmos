// Devin subscription — models from the signed-in plan; Cosmos runs the tools.
// Auth: Devin CLI credentials.toml. Completions: Codeium GetChatMessage with
// our tool schemas. We never spawn `devin acp`.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import zlib from "node:zlib";

import type { ModelDef } from "./catalog";
import { hostOf } from "./catalog";
import { newId } from "./proto";

const CACHE_TTL_MS = 45_000;
const DEFAULT_SERVER = "https://server.codeium.com";
const SESSION_PREFIX = "devin-session-token$";
const CLI_VERSION = "3000.11.3";
const CHAT_PATH = "/exa.api_server_pb.ApiServerService/GetChatMessage";
const MODELS_PATH = "/exa.api_server_pb.ApiServerService/GetCliModelConfigs";
const MAX_FRAME = 16 * 1024 * 1024;

type Cache = { at: number; ready: boolean; detail: string; models: ModelDef[] };
let cache: Cache | null = null;

type Creds = { key: string; server: string };

function extraPaths(): string[] {
  const home = os.homedir();
  const local = process.env.LOCALAPPDATA ?? "";
  const names = process.platform === "win32" ? ["devin.exe", "devin.cmd", "devin"] : ["devin"];
  const dirs = [
    path.join(home, ".local", "bin"),
    local ? path.join(local, "devin", "cli", "bin") : "",
    local ? path.join(local, "Programs", "Devin", "bin") : "",
    local
      ? path.join(local, "Programs", "Devin", "resources", "app", "extensions", "windsurf", "devin", "bin")
      : "",
    "/opt/homebrew/bin",
    "/usr/local/bin",
  ].filter(Boolean);
  const out: string[] = [];
  for (const d of dirs) for (const n of names) out.push(path.join(d, n));
  return out;
}

export function executable(): string | null {
  const override = process.env.DEVIN_EXECUTABLE;
  if (override && fs.existsSync(override) && fs.statSync(override).isFile()) return override;
  const names = process.platform === "win32" ? ["devin.exe", "devin.cmd", "devin"] : ["devin"];
  for (const dir of (process.env.PATH ?? "").split(path.delimiter)) {
    for (const n of names) {
      const p = path.join(dir, n);
      try {
        if (fs.existsSync(p) && fs.statSync(p).isFile()) return p;
      } catch {}
    }
  }
  for (const p of extraPaths()) {
    try {
      if (fs.existsSync(p) && fs.statSync(p).isFile()) return p;
    } catch {}
  }
  return null;
}

export function credentialsFile(): string {
  if (process.env.DEVIN_CREDENTIALS) return process.env.DEVIN_CREDENTIALS;
  const home = os.homedir();
  const candidates: string[] = [];
  if (process.platform === "win32" && process.env.APPDATA) {
    candidates.push(path.join(process.env.APPDATA, "devin", "credentials.toml"));
  }
  if (process.platform === "darwin") {
    candidates.push(path.join(home, "Library", "Application Support", "devin", "credentials.toml"));
  }
  const xdg = process.env.XDG_DATA_HOME || path.join(home, ".local", "share");
  candidates.push(path.join(xdg, "devin", "credentials.toml"));
  return candidates.find((p) => {
    try {
      return fs.existsSync(p) && fs.statSync(p).isFile();
    } catch {
      return false;
    }
  }) ?? candidates[0];
}

function tomlStr(text: string, key: string): string | null {
  const re = new RegExp(`^\\s*${key}\\s*=\\s*"((?:\\\\.|[^"\\\\])*)"`, "m");
  const m = text.match(re);
  if (!m) return null;
  return m[1].replace(/\\([ntr"\\]|.)/g, (_, e) => {
    if (e === "n") return "\n";
    if (e === "t") return "\t";
    if (e === "r") return "\r";
    if (e === '"' || e === "\\") return e;
    return e;
  });
}

function trustedServer(url: string): boolean {
  const host = hostOf(url).toLowerCase();
  return (
    host === "server.codeium.com" ||
    host.endsWith(".codeium.com") ||
    host === "api.devin.ai" ||
    host.endsWith(".devin.ai") ||
    host.endsWith(".windsurf.com")
  );
}

function normalizeKey(k: string): string {
  return k.startsWith(SESSION_PREFIX) ? k : SESSION_PREFIX + k;
}

export function loadCreds(): Creds | null {
  let text = "";
  try {
    text = fs.readFileSync(credentialsFile(), "utf8");
  } catch {
    return null;
  }
  const key = tomlStr(text, "windsurf_api_key") || tomlStr(text, "devin_api_key");
  if (!key) return null;
  const server = (tomlStr(text, "api_server_url") || DEFAULT_SERVER).replace(/\/+$/, "");
  if (!trustedServer(server)) return null;
  return { key: normalizeKey(key), server };
}

export function signedIn(): boolean {
  return !!loadCreds();
}

export function parseList(raw: string): ModelDef[] {
  const catalog = JSON.parse(raw) as { families?: { variants?: { model_uid?: string; label?: string }[] }[] };
  const models: ModelDef[] = [];
  const seen = new Set<string>();
  for (const f of catalog.families ?? []) {
    for (const v of f.variants ?? []) {
      const id = String(v.model_uid ?? "").trim();
      const label = String(v.label ?? "").trim();
      if (!id || seen.has(id)) continue;
      seen.add(id);
      models.push({ id, name: label || id, context: 200_000, output: 32_000, reasoning: true, price: [0, 0, 0, 0] });
    }
  }
  if (!models.length) throw new Error("Devin returned an empty model list — is the CLI signed in?");
  return models;
}

export function parseCliModelConfigs(v: any): ModelDef[] {
  const items: any[] = v?.clientModelConfigs ?? v?.client_model_configs ?? [];
  const models: ModelDef[] = [];
  const seen = new Set<string>();
  for (const m of items) {
    const id = String(m?.modelUid ?? m?.model_uid ?? "").trim();
    const label = String(m?.label ?? "").trim();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    const name = label || id;
    const max = Number(m?.maxTokens ?? m?.max_tokens ?? 0);
    models.push({
      id,
      name,
      context: 200_000,
      output: max > 0 ? max : 32_000,
      reasoning: m?.modelInfo?.modelFeatures?.supportsThinking ?? true,
      price: [0, 0, 0, 0],
    });
  }
  return models;
}

function metadata(key: string) {
  return {
    apiKey: key,
    ideName: "devin-cli",
    ideVersion: CLI_VERSION,
    extensionName: "chisel",
    extensionVersion: CLI_VERSION,
    locale: "en",
    os: process.platform === "win32" ? "windows" : process.platform === "darwin" ? "macos" : process.platform,
    ideType: "chisel",
  };
}

async function httpModels(creds: Creds, signal?: AbortSignal): Promise<ModelDef[]> {
  const resp = await fetch(creds.server + MODELS_PATH, {
    method: "POST",
    headers: { "content-type": "application/json", "connect-protocol-version": "1" },
    body: JSON.stringify({ metadata: metadata(creds.key) }),
    signal,
  });
  if (resp.status === 401 || resp.status === 403) {
    throw new Error(`Devin rejected the subscription (${resp.status}) — run \`devin auth login\``);
  }
  if (!resp.ok) throw new Error(`Devin model list failed (${resp.status})`);
  return parseCliModelConfigs(await resp.json());
}

function unionModels(a: ModelDef[], b: ModelDef[]): ModelDef[] {
  const seen = new Set<string>();
  const out: ModelDef[] = [];
  for (const m of [...a, ...b]) {
    if (!m.id || seen.has(m.id)) continue;
    seen.add(m.id);
    out.push({ ...m, name: m.name || m.id });
  }
  return out;
}

function cliModels(): ModelDef[] {
  const exe = executable();
  if (!exe) throw new Error("Devin CLI is not installed");
  const r = spawnSync(exe, ["models", "list", "--format", "json"], {
    encoding: "utf8",
    windowsHide: true,
    timeout: 15_000,
    maxBuffer: 16 * 1024 * 1024,
  });
  if (r.status !== 0) {
    const err = String(r.stderr ?? r.stdout ?? "")
      .split(/\r?\n/)
      .find((l) => l.trim()) ?? "not signed in";
    throw new Error(err.slice(0, 80));
  }
  return parseList(r.stdout ?? "");
}

function probe(): Cache {
  const creds = loadCreds();
  if (creds) {
    try {
      // sync fallback: CLI. HTTP is used by fetchModels().
      const models = cliModels();
      return { at: Date.now(), ready: true, detail: `signed in · ${models.length} models`, models };
    } catch {
      return { at: Date.now(), ready: true, detail: "signed in", models: [] };
    }
  }
  try {
    const models = cliModels();
    return { at: Date.now(), ready: true, detail: `signed in · ${models.length} models`, models };
  } catch (e: any) {
    return { at: Date.now(), ready: false, detail: String(e?.message ?? e).slice(0, 80), models: [] };
  }
}

function cached(): Cache {
  if (cache && Date.now() - cache.at < CACHE_TTL_MS) return cache;
  cache = probe();
  return cache;
}

export function invalidate() {
  cache = null;
}

export function knownReady(): boolean {
  if (signedIn()) return true;
  if (cache?.ready) return true;
  return false;
}

export function peek(): Cache | null {
  return cache;
}

export function ready() {
  return signedIn() || cached().ready;
}

export function detail() {
  if (cache?.detail) return cache.detail;
  if (signedIn()) return "signed in";
  return "run `devin auth login`";
}

export function models(): ModelDef[] {
  invalidate();
  const c = cached();
  if (!c.ready) throw new Error(c.detail);
  return c.models;
}

export async function fetchModels(signal?: AbortSignal): Promise<ModelDef[]> {
  const creds = loadCreds();
  let http: ModelDef[] = [];
  if (creds) {
    try {
      http = await httpModels(creds, signal);
    } catch (e) {
      if (signal?.aborted) throw e;
    }
  }
  let cli: ModelDef[] = [];
  try {
    cli = cliModels();
  } catch {}
  const models = unionModels(http, cli);
  if (!models.length) throw new Error("Devin returned an empty model list — is the subscription signed in?");
  cache = { at: Date.now(), ready: true, detail: `signed in · ${models.length} models`, models };
  return models;
}

export function hint() {
  if (signedIn()) return "Devin is signed in. /providers → Devin to load the live model list.";
  if (!executable()) {
    return "Devin isn't signed in. Windows: irm https://static.devin.ai/cli/setup.ps1 | iex — then `devin auth login`.";
  }
  return `Devin CLI found but not signed in (${detail()}) — run \`devin auth login\` in a terminal, then /providers again.`;
}

function encodeFrame(payload: Buffer): Buffer {
  const out = Buffer.alloc(5 + payload.length);
  out[0] = 0;
  out.writeUInt32BE(payload.length, 1);
  payload.copy(out, 5);
  return out;
}

export function decodeConnectJson(buf: Buffer): any[] {
  const out: any[] = [];
  let i = 0;
  while (i + 5 <= buf.length) {
    const flags = buf[i];
    const len = buf.readUInt32BE(i + 1);
    if (len > MAX_FRAME) throw new Error("Devin Connect frame too large");
    if (i + 5 + len > buf.length) throw new Error("Devin Connect stream ended with a partial frame");
    let payload: Buffer = buf.subarray(i + 5, i + 5 + len);
    i += 5 + len;
    if (flags & 1) payload = zlib.gunzipSync(payload);
    if (flags & 2) {
      try {
        const v = JSON.parse(payload.toString("utf8"));
        const err = v?.error;
        if (err && (err.code || err.message)) {
          throw new Error(`Devin stream error${err.code ? ` ${err.code}` : ""}: ${err.message ?? ""}`);
        }
      } catch (e) {
        if (e instanceof Error && e.message.startsWith("Devin stream")) throw e;
      }
      continue;
    }
    if (!payload.length) continue;
    try {
      out.push(JSON.parse(payload.toString("utf8")));
    } catch {}
  }
  return out;
}

function chatPrompts(req: Record<string, any>): any[] {
  const msgs: any[] = req.messages ?? [];
  const cascade = String(req.session_id || "cosmos");
  const prompts: any[] = [];
  let idx = 0;
  for (const m of msgs) {
    const role = m?.role;
    const content: any[] = m?.content ?? [];
    if (role === "assistant") {
      let text = "";
      const toolCalls: any[] = [];
      for (const b of content) {
        if (b?.type === "text") text += (text ? "\n" : "") + String(b.text ?? "");
        else if (b?.type === "tool_use") {
          toolCalls.push({
            id: b.id,
            name: b.name,
            argumentsJson: JSON.stringify(b.input ?? {}),
          });
        }
      }
      idx++;
      const p: any = { messageId: `${cascade}-${idx}`, source: 2, prompt: text };
      if (toolCalls.length) p.toolCalls = toolCalls;
      prompts.push(p);
      continue;
    }
    for (const b of content) {
      if (b?.type === "text" && b.text) {
        idx++;
        prompts.push({ messageId: `${cascade}-${idx}`, source: 1, prompt: String(b.text) });
      } else if (b?.type === "tool_result") {
        idx++;
        prompts.push({
          messageId: `${cascade}-${idx}`,
          source: 4,
          prompt: String(b.content ?? ""),
          toolCallId: b.tool_use_id,
          toolResultIsError: !!b.is_error,
        });
      }
    }
  }
  return prompts;
}

type Sink = (c: Record<string, any>) => void;

export async function complete(
  cfg: Record<string, any>,
  req: Record<string, any>,
  sink: Sink,
  signal?: AbortSignal,
): Promise<void> {
  const creds = loadCreds();
  if (!creds) throw new Error(hint());
  const tools = (req.tools ?? []).map((t: any) => ({
    name: t.name,
    description: t.description,
    jsonSchemaString: JSON.stringify(t.input_schema ?? t.parameters ?? {}),
    strict: false,
  }));
  const body: any = {
    metadata: metadata(creds.key),
    prompt: (req.system ?? []).join("\n\n"),
    chatMessagePrompts: chatPrompts(req),
    chatModelUid: String(cfg.model ?? "").trim(),
    requestType: 5,
    plannerMode: 1,
    disableParallelToolCalls: true,
    cascadeId: req.session_id || newId(),
    executionId: newId(),
    systemPromptCacheOptions: { type: 1 },
    configuration: {
      numCompletions: "1",
      maxTokens: String(req.max_tokens ?? 4096),
      maxNewlines: "200",
      temperature: 0.4,
      firstTemperature: 0.4,
      topK: "50",
      topP: 1,
      stopPatterns: ["<|user|>", "<|bot|>", "<|context_request|>", "<|endoftext|>", "<|end_of_turn|>"],
      fimEotProbThreshold: 1,
    },
  };
  if (tools.length) {
    body.tools = tools;
    body.toolChoice = { optionName: "auto" };
  }
  if (!body.chatMessagePrompts.length) throw new Error("nothing to send to Devin");

  const resp = await fetch(creds.server + CHAT_PATH, {
    method: "POST",
    headers: {
      "content-type": "application/connect+json",
      "connect-protocol-version": "1",
    },
    body: new Uint8Array(encodeFrame(Buffer.from(JSON.stringify(body)))),
    signal,
  });
  const buf = Buffer.from(await resp.arrayBuffer());
  if (resp.status === 401 || resp.status === 403) {
    throw new Error(`Devin rejected the subscription (${resp.status}) — run \`devin auth login\``);
  }
  if (!resp.ok) {
    throw new Error(`Devin chat failed (${resp.status}): ${buf.toString("utf8").slice(0, 200)}`);
  }
  const frames = decodeConnectJson(buf);
  const acc: { id: string; name: string; args: string }[] = [];
  const usage = { input: 0, cache_read: 0, cache_write: 0, output: 0 };
  const num = (v: any) => {
    const n = Number(v);
    return Number.isFinite(n) && n > 0 ? n : 0;
  };
  for (const v of frames) {
    const t = v?.deltaText ?? v?.delta_text;
    if (t) sink({ type: "text", text: String(t) });
    for (const c of v?.deltaToolCalls ?? v?.delta_tool_calls ?? []) {
      const id = String(c?.id ?? "");
      const name = String(c?.name ?? "");
      const args = String(c?.argumentsJson ?? c?.arguments_json ?? "");
      if (id) {
        const prev = acc.find((x) => x.id === id);
        if (prev) {
          if (name) prev.name = name;
          prev.args = args.startsWith(prev.args) ? args : prev.args + args;
        } else acc.push({ id, name, args });
      } else if (acc.length) {
        const prev = acc[acc.length - 1];
        if (name) prev.name = name;
        prev.args += args;
      }
    }
    const u = v?.usage;
    if (u) {
      usage.input = num(u.inputTokens ?? u.input_tokens) || usage.input;
      usage.output = num(u.outputTokens ?? u.output_tokens) || usage.output;
      usage.cache_read = num(u.cacheReadTokens ?? u.cache_read_tokens) || usage.cache_read;
      usage.cache_write = num(u.cacheWriteTokens ?? u.cache_write_tokens) || usage.cache_write;
    }
  }
  for (const t of acc) {
    if (!t.name) continue;
    let input: any = {};
    try {
      input = t.args ? JSON.parse(t.args) : {};
    } catch {
      input = {};
    }
    sink({ type: "tool_use", id: t.id || newId(), name: t.name, input });
  }
  if (!usage.output && !usage.input) usage.output = 1;
  sink({ type: "done", usage });
}
