// Providers floated to the top of the BYOK lists (onboarding and /connect),
// with a one-line pitch and where to get a key — the same order zcli uses.
import type { ProviderDef } from "./catalog";

export const PINNED: { id: string; pitch: string; url: string }[] = [
  { id: "chatgpt", pitch: "use your Plus / Pro plan — no API key", url: "" },
  { id: "openrouter", pitch: "one key, 300+ models from every lab", url: "https://openrouter.ai/keys" },
  { id: "router9", pitch: "your local 9Router — routes the subscriptions you have", url: "http://localhost:20128 → API keys" },
  { id: "anthropic", pitch: "Claude", url: "https://console.anthropic.com/settings/keys" },
  { id: "openai", pitch: "GPT and o-series", url: "https://platform.openai.com/api-keys" },
  { id: "google", pitch: "Gemini", url: "https://aistudio.google.com/apikey" },
  { id: "xai", pitch: "Grok", url: "https://console.x.ai" },
  { id: "meta", pitch: "Muse", url: "" },
  { id: "groq", pitch: "very fast open models", url: "https://console.groq.com/keys" },
  { id: "deepseek", pitch: "DeepSeek V and R", url: "https://platform.deepseek.com/api_keys" },
  { id: "mistral", pitch: "Mistral and Codestral", url: "https://console.mistral.ai/api-keys" },
  { id: "xiaomi", pitch: "MiMo", url: "" },
  { id: "moonshotai", pitch: "Kimi", url: "https://platform.moonshot.ai" },
  { id: "zai", pitch: "GLM", url: "https://z.ai/manage-apikey" },
  { id: "cerebras", pitch: "fastest inference", url: "https://cloud.cerebras.ai" },
  { id: "minimax", pitch: "MiniMax", url: "" },
];

export function pitchFor(p: ProviderDef): string {
  return PINNED.find((x) => x.id === p.id)?.pitch ?? p.env[0] ?? "";
}

export function pinRank(id: string): number {
  const i = PINNED.findIndex((x) => x.id === id);
  return i < 0 ? PINNED.length : i;
}
