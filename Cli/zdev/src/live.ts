// Live model lists, straight from each provider's own API — a port of
// zcli's zcore/src/live.rs. Asks the provider what the key in hand can use
// right now (`GET /models` in each wire dialect), keeps tool-capable chat
// models, orders them best-first (catalog order for known models, newest
// first for the rest) and fills in context and prices.
//
// Results are cached in COSMOS_CLI_HOME/live_models.json — the same file
// zcli reads — so both CLIs' /model pickers show the fetched list.
import fs from "node:fs";
import path from "node:path";

import { hostOf, isSubscription, model as catalogModel, type ModelDef, type ProviderDef } from "./catalog";
import * as creds from "./creds";
import * as devin from "./devin";
import { fresh } from "./oauth";
import { cliHome } from "./proto";

type Raw = {
  id: string;
  name?: string;
  context?: number;
  output?: number;
  price?: [number, number];
  created: number;
  reasoning: boolean;
};

export class RejectedKey extends Error {}

function key(def: ProviderDef): string {
  const k = creds.keyFor(def.id, def.env);
  if (!k) throw new Error(`no ${def.name} key — connect it first`);
  return k[0];
}

async function getJson(url: string, headers: Record<string, string>, signal?: AbortSignal): Promise<any> {
  const host = hostOf(url);
  let resp: Response;
  try {
    resp = await fetch(url, {
      headers,
      signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(20_000)]) : AbortSignal.timeout(20_000),
    });
  } catch (e: any) {
    throw new Error(`couldn't reach ${host}${e?.name === "TimeoutError" ? " (timed out)" : ""}`);
  }
  const text = await resp.text().catch(() => "");
  const s = resp.status;
  if (s >= 200 && s < 300) {
    try {
      return JSON.parse(text);
    } catch {
      throw new Error(`${host} sent something that isn't JSON`);
    }
  }
  if (s === 401 || s === 403) throw new RejectedKey(`${host} rejected the key (${s})`);
  if (s === 404) throw new Error(`${host} has no model list endpoint (404)`);
  if (s === 429) throw new Error(`${host} is rate limiting (429) — try again in a moment`);
  let msg = text.slice(0, 160);
  try {
    const v = JSON.parse(text);
    msg = v?.error?.message ?? (typeof v?.error === "string" ? v.error : v?.message) ?? msg;
  } catch {}
  throw new Error(`${host} answered ${s}: ${String(msg).trim()}`);
}

const num = (v: any): number | undefined => {
  const n = typeof v === "number" ? v : undefined;
  return n && n > 0 ? Math.floor(n) : undefined;
};

/** Per-token price strings (OpenRouter) → USD per 1M tokens. */
export function perM(v: any): number | undefined {
  const f = typeof v === "number" ? v : typeof v === "string" ? parseFloat(v) : NaN;
  return Number.isFinite(f) && f >= 0 ? Math.round(f * 1e6 * 10_000) / 10_000 : undefined;
}

async function openai(def: ProviderDef, signal?: AbortSignal): Promise<Raw[]> {
  const base = def.base_url.replace(/\/+$/, "");
  let headers: Record<string, string> = {};
  try {
    headers = { authorization: `Bearer ${key(def)}` };
  } catch (e) {
    // OpenRouter's list is public; everyone else needs the key
    if (def.id !== "openrouter") throw e;
  }
  if (def.id === "openrouter" && headers.authorization) {
    await getJson(`${base}/key`, headers, signal); // the list is public — check the key itself
  }
  const v = await getJson(`${base}/models`, headers, signal);
  const items: any[] = v?.data ?? v?.models ?? (Array.isArray(v) ? v : []);
  return items
    .filter((m) => !Array.isArray(m?.supported_parameters) || m.supported_parameters.includes("tools"))
    .filter((m) => typeof m?.id === "string")
    .map((m) => {
      const i = perM(m?.pricing?.prompt);
      const o = perM(m?.pricing?.completion);
      return {
        id: m.id,
        name: m.name ?? m.display_name,
        context: num(m.context_length) ?? num(m.context_window) ?? num(m?.top_provider?.context_length),
        output: num(m?.top_provider?.max_completion_tokens) ?? num(m.max_output_tokens),
        price: i !== undefined && o !== undefined ? [i, o] : undefined,
        created: typeof m.created === "number" ? m.created : 0,
        reasoning: Array.isArray(m.supported_parameters) && m.supported_parameters.includes("reasoning"),
      } as Raw;
    });
}

const dateKey = (s: string) => parseInt(s.replace(/\D/g, "").slice(0, 8), 10) || 0;

async function anthropic(def: ProviderDef, signal?: AbortSignal): Promise<Raw[]> {
  const base = def.base_url.replace(/\/+$/, "");
  const v = await getJson(`${base}/v1/models?limit=1000`, { "x-api-key": key(def), "anthropic-version": "2023-06-01" }, signal);
  return (v?.data ?? [])
    .filter((m: any) => typeof m?.id === "string")
    .map((m: any) => ({ id: m.id, name: m.display_name, created: m.created_at ? dateKey(m.created_at) : 0, reasoning: false }));
}

async function google(def: ProviderDef, signal?: AbortSignal): Promise<Raw[]> {
  const base = def.base_url.replace(/\/+$/, "");
  const v = await getJson(`${base}/v1beta/models?pageSize=1000`, { "x-goog-api-key": key(def) }, signal);
  return (v?.models ?? [])
    .filter((m: any) => Array.isArray(m?.supportedGenerationMethods) && m.supportedGenerationMethods.includes("generateContent"))
    .map((m: any) => ({
      id: String(m.name ?? "").replace(/^models\//, ""),
      name: m.displayName,
      context: num(m.inputTokenLimit),
      output: num(m.outputTokenLimit),
      created: 0,
      reasoning: !!m.thinking,
    }))
    .filter((m: Raw) => m.id);
}

async function devinModels(): Promise<Raw[]> {
  const models = await devin.fetchModels();
  return models.map((m) => ({
    id: m.id,
    name: m.name,
    context: m.context,
    output: m.output,
    created: 0,
    reasoning: m.reasoning,
  }));
}

async function chatgpt(signal?: AbortSignal): Promise<Raw[]> {
  const t = await fresh();
  if (!t) throw new Error("not signed in to ChatGPT");
  const v = await getJson(
    "https://chatgpt.com/backend-api/codex/models?client_version=1.0.0",
    { authorization: `Bearer ${t.access}`, "chatgpt-account-id": t.account_id, originator: "zdev" },
    signal,
  );
  const items: any[] = v?.models ?? v?.data ?? [];
  return items
    .filter((m) => m?.visibility !== "hide")
    .map((m) => ({ id: m.slug ?? m.id, name: m.display_name, context: num(m.context_window), created: 0, reasoning: true }))
    .filter((m) => typeof m.id === "string");
}

const SKIP = [
  "embed", "tts", "whisper", "dall-e", "davinci", "babbage", "moderation", "rerank", "image",
  "audio", "speech", "transcribe", "realtime", "sora", "guard", "-search", "computer-use", "veo",
  "imagen", "lyria", "aqa", "native-audio", "clip", "stable-diffusion", "flux",
];

/** Not a chat model an agent can drive. */
export function noise(id: string): boolean {
  const l = id.toLowerCase();
  return l.endsWith(":batch") || SKIP.some((s) => l.includes(s));
}

/** Filter, enrich from the catalog and order best-first. */
export function merge(def: ProviderDef, raw: Raw[]): ModelDef[] {
  const seen = new Set<string>();
  const list = raw.filter((r) => (isSubscription(def) || !noise(r.id)) && !seen.has(r.id) && seen.add(r.id));
  const rank = (id: string) => {
    const i = def.models.findIndex((x) => x.id === id);
    return i < 0 ? null : i;
  };
  list.sort((a, b) => {
    const [x, y] = [rank(a.id), rank(b.id)];
    if (x !== null && y !== null) return x - y;
    if (x !== null) return -1;
    if (y !== null) return 1;
    return b.created - a.created || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
  });
  return list.map((r) => {
    const known = catalogModel(def, r.id);
    return {
      id: r.id,
      name: r.name ?? known?.name ?? r.id,
      context: r.context ?? known?.context ?? 128_000,
      output: r.output ?? known?.output ?? 8_192,
      reasoning: r.reasoning || !!known?.reasoning,
      price: r.price ? [r.price[0], r.price[1], 0, 0] : (known?.price ?? [0, 0, 0, 0]),
    };
  });
}

/** Ask `def`'s API for the models this machine's credentials can use. */
export async function fetchModels(def: ProviderDef, signal?: AbortSignal): Promise<ModelDef[]> {
  const raw =
    def.kind === "chatgpt"
      ? await chatgpt(signal)
      : def.kind === "devin"
        ? await devinModels()
        : def.kind === "anthropic"
          ? await anthropic(def, signal)
          : def.kind === "google"
            ? await google(def, signal)
            : await openai(def, signal);
  const models = merge(def, raw);
  if (!models.length) throw new Error(`${hostOf(def.base_url)} returned no chat models`);
  return models;
}

// ── cache ───────────────────────────────────────────────────────────────────

const cachePath = () => path.join(cliHome(), "live_models.json");

function load(): Record<string, { at: number; models: ModelDef[] }> {
  try {
    return JSON.parse(fs.readFileSync(cachePath(), "utf8")) ?? {};
  } catch {
    return {};
  }
}

/** The last live list fetched for `provider` (by either CLI), if any. */
export function cached(provider: string): ModelDef[] | null {
  const e = load()[provider];
  return e?.models?.length ? e.models : null;
}

export function store(provider: string, models: ModelDef[]): void {
  const c = load();
  c[provider] = { at: Math.floor(Date.now() / 1000), models };
  fs.mkdirSync(cliHome(), { recursive: true });
  fs.writeFileSync(cachePath(), JSON.stringify(c), "utf8");
}
