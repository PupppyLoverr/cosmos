// Regenerate catalog.json — the built-in BYOK provider/model list.
//
//   curl -o models-dev.json https://models.dev/api.json
//   node gen.mjs models-dev.json > catalog.json
//
// Source: models.dev (the community catalog opencode and pi also build on).
// We keep tool-calling text models only, newest first, capped per provider,
// and pin each provider to the wire protocol zproviders speaks.

import { readFileSync } from "node:fs";

const src = JSON.parse(readFileSync(process.argv[2] ?? "models-dev.json", "utf8"));

// id → [display name, kind, base_url, env vars, max models]
// kind: openai (chat completions) | responses (OpenAI Responses API) |
//       anthropic (Messages API) | google (Gemini generateContent)
const PROVIDERS = [
  ["openai", "OpenAI", "responses", "https://api.openai.com/v1", ["OPENAI_API_KEY"], 14],
  ["anthropic", "Anthropic", "anthropic", "https://api.anthropic.com", ["ANTHROPIC_API_KEY"], 10],
  ["google", "Google Gemini", "google", "https://generativelanguage.googleapis.com", ["GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"], 10],
  ["xai", "xAI Grok", "openai", "https://api.x.ai/v1", ["XAI_API_KEY"], 8],
  ["meta", "Meta", "responses", "https://api.meta.ai/v1", ["META_MODEL_API_KEY", "META_API_KEY"], 6],
  ["llama", "Meta Llama API", "openai", "https://api.llama.com/compat/v1", ["LLAMA_API_KEY"], 8],
  ["xiaomi", "Xiaomi MiMo", "openai", "https://api.xiaomimimo.com/v1", ["XIAOMI_API_KEY"], 9],
  ["alibaba", "Alibaba Qwen (Bailian)", "openai", "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", ["DASHSCOPE_API_KEY"], 12],
  ["alibaba-cn", "Alibaba Qwen (Bailian, China)", "openai", "https://dashscope.aliyuncs.com/compatible-mode/v1", ["DASHSCOPE_API_KEY"], 12],
  ["bailing", "Ant Bailing (Ling)", "openai", "https://api.tbox.cn/api/llm/v1", ["BAILING_API_TOKEN"], 6],
  ["deepseek", "DeepSeek", "openai", "https://api.deepseek.com/v1", ["DEEPSEEK_API_KEY"], 6],
  ["moonshotai", "Moonshot Kimi", "openai", "https://api.moonshot.ai/v1", ["MOONSHOT_API_KEY"], 6],
  ["zai", "Z.ai GLM", "openai", "https://api.z.ai/api/paas/v4", ["ZAI_API_KEY", "ZHIPU_API_KEY"], 8],
  ["zhipuai", "Zhipu GLM (China)", "openai", "https://open.bigmodel.cn/api/paas/v4", ["ZHIPU_API_KEY"], 8],
  ["minimax", "MiniMax", "anthropic", "https://api.minimax.io/anthropic", ["MINIMAX_API_KEY"], 6],
  ["mistral", "Mistral", "openai", "https://api.mistral.ai/v1", ["MISTRAL_API_KEY"], 10],
  ["groq", "Groq", "openai", "https://api.groq.com/openai/v1", ["GROQ_API_KEY"], 8],
  ["cerebras", "Cerebras", "openai", "https://api.cerebras.ai/v1", ["CEREBRAS_API_KEY"], 6],
  ["togetherai", "Together AI", "openai", "https://api.together.xyz/v1", ["TOGETHER_API_KEY"], 12],
  ["fireworks-ai", "Fireworks AI", "openai", "https://api.fireworks.ai/inference/v1", ["FIREWORKS_API_KEY"], 12],
  ["openrouter", "OpenRouter", "openai", "https://openrouter.ai/api/v1", ["OPENROUTER_API_KEY"], 24],
  ["vercel", "Vercel AI Gateway", "openai", "https://ai-gateway.vercel.sh/v1", ["AI_GATEWAY_API_KEY"], 16],
  ["nvidia", "NVIDIA NIM", "openai", "https://integrate.api.nvidia.com/v1", ["NVIDIA_API_KEY"], 12],
  ["deepinfra", "DeepInfra", "openai", "https://api.deepinfra.com/v1/openai", ["DEEPINFRA_API_KEY"], 12],
  ["huggingface", "Hugging Face", "openai", "https://router.huggingface.co/v1", ["HF_TOKEN"], 12],
  ["cohere", "Cohere", "openai", "https://api.cohere.ai/compatibility/v1", ["COHERE_API_KEY"], 6],
  ["stepfun", "StepFun", "openai", "https://api.stepfun.com/v1", ["STEPFUN_API_KEY"], 6],
  ["siliconflow", "SiliconFlow", "openai", "https://api.siliconflow.com/v1", ["SILICONFLOW_API_KEY"], 12],
  ["novita-ai", "Novita AI", "openai", "https://api.novita.ai/openai", ["NOVITA_API_KEY"], 12],
  ["ollama-cloud", "Ollama Cloud", "openai", "https://ollama.com/v1", ["OLLAMA_API_KEY"], 10],
];

const round = (n) => (typeof n === "number" ? Math.round(n * 10000) / 10000 : 0);

const out = [];
for (const [id, name, kind, base_url, env, cap] of PROVIDERS) {
  const p = src[id];
  if (!p) {
    console.error(`skip ${id}: not in models.dev`);
    continue;
  }
  const models = Object.values(p.models ?? {})
    .filter((m) => m.tool_call && (m.modalities?.input ?? ["text"]).includes("text"))
    .filter((m) => !m.status || m.status !== "deprecated")
    .sort((a, b) => String(b.release_date ?? "").localeCompare(String(a.release_date ?? "")))
    .slice(0, cap)
    .map((m) => ({
      id: m.id,
      name: m.name ?? m.id,
      context: m.limit?.context ?? 128000,
      output: m.limit?.output ?? 8192,
      reasoning: !!m.reasoning,
      price: [round(m.cost?.input), round(m.cost?.output), round(m.cost?.cache_read), round(m.cost?.cache_write)],
    }));
  out.push({ id, name, kind, base_url, env, models });
}
process.stdout.write(JSON.stringify({ source: "models.dev", generated: new Date().toISOString().slice(0, 10), providers: out }, null, 1) + "\n");
