// Live model discovery — after BYOK setup, hit the provider's /models
// endpoint with the fresh key and let the user pick a real model id.
// URLs come from the BYOK catalog (declared constants), never user input.
import type { ByokProvider } from "./byok";

const TIMEOUT = 10_000;

async function getJson(url: string, headers: Record<string, string>) {
  const resp = await fetch(url, {
    headers,
    signal: AbortSignal.timeout(TIMEOUT),
  });
  if (!resp.ok) throw new Error(`${resp.status}`);
  return resp.json();
}

/** Ids of chat-capable models, best-first where the API provides order. */
export async function fetchModels(
  p: ByokProvider,
  key: string,
): Promise<string[]> {
  const base = p.base_url.replace(/\/+$/, "");
  switch (p.kind) {
    case "openai": {
      // OpenAI-compatible /models — covers groq, deepseek, mistral, meta,
      // mimo, moonshot, zhipu, baidu, together, openrouter, 9router, ollama.
      const v: any = await getJson(`${base}/models`, {
        authorization: `Bearer ${key || "none"}`,
      });
      const ids = (v?.data ?? [])
        .map((m: any) => m?.id ?? m?.name)
        .filter((s: any) => typeof s === "string") as string[];
      if (!ids.length) throw new Error("empty model list");
      return ids;
    }
    case "anthropic": {
      const v: any = await getJson(`${base}/v1/models`, {
        "x-api-key": key,
        "anthropic-version": "2023-06-01",
      });
      const ids = (v?.data ?? [])
        .map((m: any) => m?.id)
        .filter((s: any) => typeof s === "string") as string[];
      if (!ids.length) throw new Error("empty model list");
      return ids;
    }
    case "google": {
      const v: any = await getJson(`${base}/v1beta/models`, {
        "x-goog-api-key": key,
      });
      const ids = (v?.models ?? [])
        .map((m: any) => String(m?.name ?? "").replace(/^models\//, ""))
        .filter(
          (s: string) =>
            s &&
            // keep generation models, drop embedders/encoders
            /gemini|gemma|learnlm/i.test(s),
        ) as string[];
      if (!ids.length) throw new Error("empty model list");
      return ids;
    }
    case "chatgpt":
      // subscription backend has no /models — the Codex lineup is fixed
      return ["gpt-5.1-codex", "gpt-5.1-codex-mini", "gpt-5.1-codex-max"];
    default:
      return [p.model];
  }
}
