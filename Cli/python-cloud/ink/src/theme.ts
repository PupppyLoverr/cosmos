// Palette + glyphs ported from the brainless claude-* components.
// brainless uses DOM/CSS; these are the same colors/glyphs as Ink props.
export const ROSE = "#cd694a"; // terracotta accent (logo, borders, active)
export const ROSE_HI = "#e79475"; // shimmer highlight
export const GRAY = "#949494";
export const DIM = "#7d7d7d";
export const RULE = "#808080";
export const GREEN = "#4ea96f"; // tool success / diff add
export const RED = "#f7768e"; // tool error / diff del
export const YELLOW = "#e0af68"; // pending
export const ACTIVE = "#afd7ff"; // selected slash row
export const MODE_COLOR: Record<string, string> = {
  agent: "#ffd700", // ⏵⏵ gold
  plan: "#5fafaf", // ⏸ teal
  yolo: "#f7768e", // ⏵⏵ red — no guardrails
};

export const GLYPHS = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];
export const VERBS = [
  "Thinking",
  "Levitating",
  "Schlepping",
  "Herding",
  "Percolating",
  "Noodling",
  "Conjuring",
];
