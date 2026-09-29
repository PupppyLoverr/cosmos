import type { Usage } from "./types";

export function formatCost(u: Usage, now = Date.now()): string {
  const lines = [
    "**Cost**",
    `Input: ${u.input.toLocaleString()}`,
    `Output: ${u.output.toLocaleString()}`,
    `Cache read: ${u.cache_read.toLocaleString()}`,
    `Cache write: ${u.cache_write.toLocaleString()}`,
    `USD total: ${u.cum_usd == null ? "unpriced" : `$${u.cum_usd.toFixed(4)}`}`,
    `Turns: ${u.turns}`,
    `Wall time: ${formatWall((now - u.started_at) / 1000)}`,
  ];
  const models = Object.entries(u.models);
  if (models.length > 1) {
    lines.push(...models.map(([model, usd]) => `${model}: $${usd.toFixed(4)}`));
  }
  return lines.join("\n");
}

function formatWall(seconds: number): string {
  return seconds < 60 ? `${seconds.toFixed(1)}s` : `${(seconds / 60).toFixed(1)}m`;
}

export function costFormatterTestValue(): string {
  return formatCost({
    input: 100,
    output: 20,
    cache_read: 30,
    cache_write: 4,
    usd: 0.15,
    cum_usd: 0.15,
    turns: 2,
    started_at: 0,
    models: { alpha: 0.12, beta: 0.03 },
  }, 61_200);
}
