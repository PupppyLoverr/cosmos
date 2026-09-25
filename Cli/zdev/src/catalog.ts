// Built-in provider catalog for bring-your-own-key use — shared with zcli.
//
// catalog.json is zcli's (rust-tui/crates/zcore/catalog, generated from
// models.dev): 30 providers with wire protocol, base URL, key env vars and
// current tool-calling models. The ChatGPT subscription, 9Router and the
// local servers are added by hand, exactly as zcore/src/catalog.rs does.
// A model is addressed as `provider/model`, e.g. `anthropic/claude-opus-5-5`.
import CATALOG from "../../rust-tui/crates/zcore/catalog/catalog.json";

export type ModelDef = {
  id: string;
  name: string;
  context: number;
  output: number;
  reasoning: boolean;
  /** USD per 1M tokens: [input, output, cache read, cache write] */
  price: [number, number, number, number];
};

export type ProviderDef = {
  id: string;
  name: string;
  /** openai | responses | anthropic | google | chatgpt */
  kind: string;
  base_url: string;
  env: string[];
  models: ModelDef[];
};

export const isSubscription = (p: ProviderDef) => p.kind === "chatgpt";

export function hostOf(url: string): string {
  const rest = url.split("://")[1] ?? "";
  return rest.split(/[/:?]/)[0] ?? "";
}

const m = (id: string, name: string, context: number, output = 128_000, reasoning = true): ModelDef => ({
  id,
  name,
  context,
  output,
  reasoning,
  price: [0, 0, 0, 0],
});

/** Models served on a ChatGPT Plus/Pro plan (the Codex CLI list). */
const CHATGPT: ProviderDef = {
  id: "chatgpt",
  name: "ChatGPT (Plus / Pro subscription)",
  kind: "chatgpt",
  base_url: "https://chatgpt.com/backend-api",
  env: [],
  models: [
    m("gpt-5.5", "GPT-5.5", 272_000),
    m("gpt-6-astra", "GPT-6 Astra", 272_000),
    m("gpt-5.6-sol", "GPT-5.6 Sol", 272_000),
    m("gpt-5.6-terra", "GPT-5.6 Terra", 272_000),
    m("gpt-5.6-luna", "GPT-5.6 Luna", 272_000),
    m("gpt-5.3-codex-spark", "GPT-5.3 Codex Spark", 128_000),
  ],
};

/** 9Router — a local OpenAI-compatible gateway over existing subscriptions. */
const ROUTER9: ProviderDef = {
  id: "router9",
  name: "9Router",
  kind: "openai",
  base_url: "http://localhost:20128/v1",
  env: ["ROUTER9_API_KEY"],
  models: [
    m("kr/claude-sonnet-4.5", "Claude Sonnet 4.5 (Kiro)", 200_000, 32_000, false),
    m("ag/gemini-3.8-flash", "Gemini 3.8 Flash (Antigravity)", 1_048_576, 32_000, false),
    m("kc/kilo-auto/free", "Kilo Auto (free)", 200_000, 32_000, false),
  ],
};

const LOCAL: ProviderDef[] = [
  { id: "ollama", name: "Ollama (local)", kind: "openai", base_url: "http://127.0.0.1:11434/v1", env: [], models: [] },
  { id: "lmstudio", name: "LM Studio (local)", kind: "openai", base_url: "http://127.0.0.1:1234/v1", env: [], models: [] },
];

let all: ProviderDef[] | null = null;

export function providers(): ProviderDef[] {
  if (!all) {
    const file = (CATALOG as any).providers as ProviderDef[];
    all = [CHATGPT, ROUTER9, ...file.map((p) => ({ ...p, env: p.env ?? [], models: p.models ?? [] })), ...LOCAL];
  }
  return all;
}

export function provider(id: string): ProviderDef | undefined {
  return providers().find((p) => p.id === id);
}

export function model(p: ProviderDef, id: string): ModelDef | undefined {
  return p.models.find((x) => x.id === id);
}

/** `provider/model` → [provider, model id]. The model part may contain
 * slashes (`openrouter/anthropic/claude-…`). */
export function splitRef(alias: string): [string, string] | null {
  const i = alias.indexOf("/");
  if (i <= 0 || i === alias.length - 1) return null;
  return [alias.slice(0, i), alias.slice(i + 1)];
}
