export const Dark = {
  bg: "#060606",
  surface: "#0D0D0D",
  raised: "#1E1E1E",
  border: "#2A2A2A",
  text: "#F5F5F5",
  muted: "#A3A3A3",
  faint: "#737373",
  accent: "#6366F1",
  accentStrong: "#4F46E5",
  danger: "#F43F5E",
  warning: "#F59E0B",
  working: "#F472B6",
  done: "#34D399",
  awaiting: "#FBBF24",
  idle: "#525252",
  isDark: true,
} as const;

export const Light = {
  bg: "#F7F7F4",
  surface: "#FFFFFF",
  raised: "#F0F0EC",
  border: "#E4E4DE",
  text: "#171717",
  muted: "#525252",
  faint: "#737373",
  accent: "#4F46E5",
  accentStrong: "#4338CA",
  danger: "#E11D48",
  warning: "#D97706",
  working: "#DB2777",
  done: "#059669",
  awaiting: "#D97706",
  idle: "#A3A3A3",
  isDark: false,
} as const;

export type Palette = typeof Dark | typeof Light;

export function palette(scheme: "light" | "dark"): Palette {
  return scheme === "light" ? Light : Dark;
}

export const indicatorColor: Record<string, string> = {
  working: Dark.working,
  awaitingInput: Dark.awaiting,
  errored: Dark.danger,
  completed: Dark.done,
  idle: Dark.idle,
};
