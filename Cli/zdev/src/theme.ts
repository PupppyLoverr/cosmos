// Palette + glyphs ported from the brainless claude-* components.
// brainless uses DOM/CSS; these are the same colors/glyphs as Ink props.
// Monochrome build — Cosmos ships black & white: whites carry signal,
// grays carry structure, inverse/bold carry emphasis.
export const ROSE = "#e8e8e8"; // accent — was terracotta; now bright white
export const ROSE_HI = "#ffffff"; // shimmer highlight — pure white
export const GRAY = "#949494";
export const DIM = "#7d7d7d";
export const RULE = "#808080";
export const GREEN = "#e0e0e0"; // success — bright gray
export const RED = "#6f6f6f"; // failure — sunken dark gray
export const YELLOW = "#b0b0b0"; // pending — mid gray
export const ACTIVE = "#ffffff"; // selected row
export const CYAN = "#d0d0d0"; // tool arg — pale gray
export const SILVER = "#c0c0c0"; // ◆ event diamond
export const BRAILLE = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
export const MODE_COLOR: Record<string, string> = {
  agent: "#ffffff", // ⏵⏵ bright
  plan: "#8a8a8a", // ⏸ dim — read-only
  yolo: "#e8e8e8", // ⏵⏵ white — no guardrails
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
