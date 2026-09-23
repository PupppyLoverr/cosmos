// Cosmos mobile theme — mirrors the desktop theme model: a theme *family*
// (Cosmos, Catppuccin, Tokyo Night, …) resolved against the appearance, an
// interactive *accent* family, a *surface* treatment (Liquid Glass / frosted /
// opaque) and an optional *backdrop* (gradient preset or the user's photo).
// Color is reserved for the accent and for status; everything else is tonal.

import React, { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import { AccessibilityInfo, Appearance, Platform, useColorScheme, type TextStyle } from 'react-native';
import * as SecureStore from 'expo-secure-store';
import { isGlassEffectAPIAvailable, isLiquidGlassAvailable } from 'expo-glass-effect';
import { wallpaperPath } from './wallpaper-store';

export type AppearancePref = 'system' | 'light' | 'dark';
export type Scheme = 'light' | 'dark';
/**
 * Surface material. Mirrors the desktop `SurfacePreference`:
 * glass = Apple Liquid Glass (iOS 26+), acrylic = thick system blur,
 * frosted = thin system blur, opaque = solid theme surfaces.
 */
export type SurfacePref = 'auto' | 'glass' | 'acrylic' | 'frosted' | 'opaque';
export type Material = 'glass' | 'acrylic' | 'frosted' | 'opaque';
export type AccentId = 'theme' | 'mono' | 'cosmos' | 'orange' | 'amber' | 'green' | 'cyan' | 'blue' | 'pink';
export type BackdropPresetId = 'aurora' | 'dusk' | 'ocean' | 'meadow' | 'ember' | 'graphite';
/** Image treatment applied to a photo wallpaper — same vocabulary as the desktop `NewThreadBackgroundEffect`. */
export type WallpaperEffect = 'none' | 'dither' | 'halftone' | 'ascii' | 'scanlines';
export type Backdrop =
  | { kind: 'none' }
  | { kind: 'preset'; id: BackdropPresetId }
  /** `uri` is the file name inside the app's wallpaper directory (resolved to a path in `Theme.backdrop`). */
  | { kind: 'photo'; uri: string; width?: number; height?: number };

export interface ThemePrefs {
  appearance: AppearancePref;
  family: string;
  accent: AccentId;
  surface: SurfacePref;
  backdrop: Backdrop;
  effect: WallpaperEffect;
  /** 0–1: how much the backdrop is dimmed toward the theme background. */
  backdropDim: number;
}

export const wallpaperEffects: { id: WallpaperEffect; label: string; description: string }[] = [
  { id: 'none', label: 'Normal', description: 'Shows the original photo.' },
  { id: 'dither', label: 'Dither', description: 'Rebuilds the photo with a dithered color palette.' },
  { id: 'halftone', label: 'Halftone', description: 'Recreates the photo with colored print dots.' },
  { id: 'ascii', label: 'ASCII', description: 'Recreates the photo with colored characters.' },
  { id: 'scanlines', label: 'Scanlines', description: 'Adds a horizontal display-line texture.' },
];

export const surfaceOptions: { id: SurfacePref; label: string; description: string }[] = [
  { id: 'auto', label: 'Auto', description: 'Liquid Glass where available, otherwise acrylic.' },
  { id: 'glass', label: 'Glass', description: 'Apple Liquid Glass — refracts what is behind it.' },
  { id: 'acrylic', label: 'Acrylic', description: 'Thick, softly tinted blur.' },
  { id: 'frosted', label: 'Frosted', description: 'Thin, light blur.' },
  { id: 'opaque', label: 'Solid', description: 'Flat theme surfaces. No transparency.' },
];

const PREFS_KEY = 'cosmos.theme.v3';
const PREV_PREFS_KEY = 'cosmos.theme.v2';
const LEGACY_APPEARANCE_KEY = 'cosmos.appearance';

export const defaultPrefs: ThemePrefs = {
  appearance: 'system',
  family: 'cosmos',
  accent: 'theme',
  surface: 'auto',
  backdrop: { kind: 'none' },
  effect: 'none',
  backdropDim: 0.35,
};

// ---------------------------------------------------------------------------
// Color utilities (hex only; the seed palette is authored in hex)

function parseHex(hex: string): [number, number, number] {
  const h = hex.replace('#', '');
  const v = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const n = parseInt(v.slice(0, 6), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
const toHex = (r: number, g: number, b: number) =>
  `#${[r, g, b].map((c) => Math.round(Math.min(255, Math.max(0, c))).toString(16).padStart(2, '0')).join('')}`;

/** Linear blend of two hex colors; `amount` is the weight of `b`. */
export function mix(a: string, b: string, amount: number): string {
  const [r1, g1, b1] = parseHex(a);
  const [r2, g2, b2] = parseHex(b);
  return toHex(r1 + (r2 - r1) * amount, g1 + (g2 - g1) * amount, b1 + (b2 - b1) * amount);
}
export function alpha(hex: string, a: number): string {
  const [r, g, b] = parseHex(hex);
  return `rgba(${r},${g},${b},${a})`;
}
function luminance(hex: string): number {
  const [r, g, b] = parseHex(hex).map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
/** White or near-black, whichever reads better on `bg`. */
export function onColor(bg: string): string {
  return luminance(bg) > 0.35 ? '#0B0B0D' : '#FFFFFF';
}

// ---------------------------------------------------------------------------
// Theme families — ported from crates/theme/src/builtins.rs (desktop).

interface Seeds {
  background: string;
  shell: string;
  raised: string;
  card: string;
  text: string;
  muted: string;
  faint: string;
  accent: string;
  danger: string;
  warning: string;
  success: string;
  frosted?: boolean;
}

export interface ThemeFamily {
  id: string;
  name: string;
  light?: Seeds;
  dark?: Seeds;
}

const s = (
  background: string, shell: string, raised: string, card: string,
  text: string, muted: string, faint: string,
  accent: string, danger: string, warning: string, success: string, frosted = false,
): Seeds => ({ background, shell, raised, card, text, muted, faint, accent, danger, warning, success, frosted });

export const families: ThemeFamily[] = [
  {
    id: 'cosmos', name: 'Cosmos',
    // Near-black / paper — the same two values as the app.json splash. Ink
    // text, one quiet link-blue accent; everything else is tonal.
    dark: s('#0B0B0C', '#111113', '#2A2A2E', '#161618', '#EDEDEF', '#A1A1A8', '#6E6E76', '#5E9EFF', '#FF6B6B', '#E8B04B', '#3DD68C', true),
    light: s('#F5F5F7', '#ECECEF', '#E4E4E8', '#FFFFFF', '#1C1C1E', '#6B6B72', '#8E8E94', '#0A66D9', '#D92D20', '#B25E09', '#16873D', true),
  },
  {
    id: 'vscode', name: 'Dark+ / Light+',
    dark: s('#1e1e1e', '#181818', '#2a2d2e', '#252526', '#d4d4d4', '#a8a8a8', '#858585', '#007acc', '#f48771', '#cca700', '#89d185'),
    light: s('#ffffff', '#f3f3f3', '#e8e8e8', '#ffffff', '#1f1f1f', '#616161', '#767676', '#0078d4', '#a1260d', '#895503', '#008000'),
  },
  {
    id: 'catppuccin', name: 'Catppuccin',
    dark: s('#1e1e2e', '#181825', '#313244', '#242436', '#cdd6f4', '#a6adc8', '#9399b2', '#cba6f7', '#f38ba8', '#f9e2af', '#a6e3a1'),
    light: s('#eff1f5', '#e6e9ef', '#dce0e8', '#f7f8fa', '#4c4f69', '#5c5f77', '#6c6f85', '#8839ef', '#d20f39', '#df8e1d', '#40a02b'),
  },
  {
    id: 'tokyo-night', name: 'Tokyo Night',
    dark: s('#1a1b26', '#16161e', '#292e42', '#1f2335', '#c0caf5', '#9aa5ce', '#7f87ad', '#7aa2f7', '#f7768e', '#e0af68', '#9ece6a'),
    light: s('#d5d6db', '#cbccd1', '#bfc0c6', '#e1e2e7', '#343b59', '#485e7d', '#5a6378', '#34548a', '#8c4351', '#8f5e15', '#33635c'),
  },
  {
    id: 'github', name: 'GitHub',
    dark: s('#0d1117', '#010409', '#21262d', '#161b22', '#e6edf3', '#8b949e', '#6e7681', '#2f81f7', '#f85149', '#d29922', '#3fb950'),
    light: s('#ffffff', '#f6f8fa', '#d0d7de', '#ffffff', '#1f2328', '#656d76', '#818b98', '#0969da', '#cf222e', '#9a6700', '#1a7f37'),
  },
  {
    id: 'ayu', name: 'Ayu',
    dark: s('#10141c', '#0d1017', '#1b1f29', '#141821', '#bfbdb6', '#8a9199', '#5a6673', '#e6b450', '#d95757', '#ffb454', '#70bf56'),
    light: s('#fcfcfc', '#f8f9fa', '#e6e9ed', '#fafafa', '#5c6166', '#828e9f', '#969da8', '#f29718', '#e65050', '#eba400', '#6cbf43'),
  },
  {
    id: 'gruvbox', name: 'Gruvbox',
    dark: s('#282828', '#1d2021', '#504945', '#32302f', '#ebdbb2', '#bdae93', '#928374', '#8ec07c', '#fb4934', '#fabd2f', '#b8bb26'),
    light: s('#fbf1c7', '#f2e5bc', '#d5c4a1', '#f9f5d7', '#3c3836', '#665c54', '#7c6f64', '#076678', '#9d0006', '#b57614', '#79740e'),
  },
  {
    id: 'rose-pine', name: 'Rosé Pine',
    dark: s('#232136', '#1f1d2e', '#393552', '#2a273f', '#e0def4', '#908caa', '#6e6a86', '#c4a7e7', '#eb6f92', '#f6c177', '#9ccfd8'),
    light: s('#faf4ed', '#f2e9e1', '#dfdad9', '#fffaf3', '#575279', '#797593', '#9893a5', '#907aa9', '#b4637a', '#ea9d34', '#56949f'),
  },
  {
    id: 'night-owl', name: 'Night Owl',
    dark: s('#011627', '#01111d', '#0b253a', '#071d2e', '#d6deeb', '#8ca6ba', '#637777', '#82aaff', '#ef5350', '#ffeb95', '#22da6e'),
    light: s('#fbfbfb', '#f0f0f0', '#d3e8f8', '#ffffff', '#403f53', '#66657a', '#7a8297', '#4876d6', '#de3d3b', '#a67d00', '#08916a'),
  },
  {
    id: 'dracula', name: 'Dracula',
    dark: s('#282a36', '#21222c', '#44475a', '#2d303e', '#f8f8f2', '#c5c8c6', '#9a9cab', '#bd93f9', '#ff5555', '#f1fa8c', '#50fa7b'),
  },
  {
    id: 'nord', name: 'Nord',
    dark: s('#2e3440', '#292e39', '#434c5e', '#3b4252', '#d8dee9', '#aeb6c4', '#7f899b', '#88c0d0', '#bf616a', '#ebcb8b', '#a3be8c'),
  },
  {
    id: 'one-dark', name: 'One Dark Pro',
    dark: s('#282c34', '#21252b', '#2c313a', '#21252b', '#abb2bf', '#9da5b4', '#7f848e', '#528bff', '#e05561', '#d19a66', '#8cc265'),
  },
  {
    id: 'palenight', name: 'Palenight',
    dark: s('#292d3e', '#232635', '#34384c', '#292d3e', '#eeffff', '#a6accd', '#697098', '#82aaff', '#ff5572', '#ffcb6b', '#c3e88d'),
  },
  {
    id: 'synthwave', name: "SynthWave '84",
    dark: s('#262335', '#241b2f', '#37294d', '#2a2139', '#ffffff', '#b8b5c6', '#848bbd', '#ff7edb', '#fe4450', '#fede5d', '#72f1b8'),
  },
  {
    id: 'cobalt2', name: 'Cobalt2',
    dark: s('#193549', '#15232d', '#20455e', '#122738', '#ffffff', '#b7c9d3', '#7da1b5', '#ffc600', '#ff628c', '#ffc600', '#3ad900'),
  },
  {
    id: 'andromeda', name: 'Andromeda',
    dark: s('#23262e', '#1e2027', '#2d313b', '#23262e', '#d5ced9', '#a6a0aa', '#746f77', '#00e8c6', '#ee5d43', '#ff9f2e', '#96e072'),
  },
];

export const familyById = (id: string): ThemeFamily => families.find((f) => f.id === id) ?? families[0]!;

// Accent families — the desktop's OKLCH pairs pre-resolved to sRGB.
// [primary, strong] per appearance; `strong` carries `onAccent` text.
const ACCENTS: Record<Exclude<AccentId, 'theme' | 'mono'>, { label: string; dark: [string, string]; light: [string, string] }> = {
  cosmos: { label: 'Stone', dark: ['#C8BCA8', '#8A7E6B'], light: ['#6E6353', '#6E6353'] },
  orange: { label: 'Orange', dark: ['#ff8918', '#bc4200'], light: ['#ae3400', '#ae3400'] },
  amber: { label: 'Amber', dark: ['#f1b200', '#8e5f00'], light: ['#825300', '#825300'] },
  green: { label: 'Green', dark: ['#4bcb71', '#00792f'], light: ['#006c28', '#006c28'] },
  cyan: { label: 'Cyan', dark: ['#00c8d8', '#007280'], light: ['#006572', '#006572'] },
  blue: { label: 'Blue', dark: ['#4c9fff', '#005dd1'], light: ['#0052cc', '#0052cc'] },
  pink: { label: 'Pink', dark: ['#f46eb4', '#b11776'], light: ['#a7016d', '#a7016d'] },
};

export const accentOptions: { id: AccentId; label: string }[] = [
  { id: 'theme', label: 'Theme' },
  { id: 'mono', label: 'Mono' },
  ...(Object.keys(ACCENTS) as (keyof typeof ACCENTS)[]).map((id) => ({ id, label: ACCENTS[id].label })),
];

/** Swatch color for an accent chip in Settings. */
export function accentSwatch(id: AccentId, seeds: Seeds, scheme: Scheme): string {
  if (id === 'theme') return seeds.accent;
  if (id === 'mono') return seeds.text;
  return ACCENTS[id][scheme][0];
}

// ---------------------------------------------------------------------------
// Backdrops

export interface BackdropPreset {
  id: BackdropPresetId;
  name: string;
  /** Gradient stops, authored for each appearance. */
  dark: string[];
  light: string[];
}

export const backdropPresets: BackdropPreset[] = [
  { id: 'aurora', name: 'Aurora', dark: ['#0b1030', '#1b1a5c', '#0f4c5c', '#060606'], light: ['#dfe6ff', '#c8d8ff', '#cdeff3', '#f5f7ff'] },
  { id: 'dusk', name: 'Dusk', dark: ['#2a0f3a', '#5b1f5e', '#b8443f', '#1a0d12'], light: ['#ffe1ec', '#ffd1c2', '#ffe8b8', '#fff5f0'] },
  { id: 'ocean', name: 'Ocean', dark: ['#04121f', '#083a5e', '#0b6a7a', '#020a12'], light: ['#d9f0ff', '#bfe3ff', '#c9f5f0', '#f0fbff'] },
  { id: 'meadow', name: 'Meadow', dark: ['#0a1a10', '#123f2a', '#2c5e2a', '#050b07'], light: ['#e3f7e6', '#cdeed4', '#f1f7c9', '#f6fff5'] },
  { id: 'ember', name: 'Ember', dark: ['#1a0a05', '#5e1f0b', '#a34a12', '#0d0503'], light: ['#ffe9d6', '#ffd4b0', '#ffe6c2', '#fff7ef'] },
  { id: 'graphite', name: 'Graphite', dark: ['#1a1b1f', '#2a2c33', '#101114', '#000000'], light: ['#eceef2', '#dfe2e8', '#f6f7f9', '#ffffff'] },
];

// ---------------------------------------------------------------------------
// Resolved theme

export interface Theme {
  scheme: Scheme;
  familyId: string;
  familyName: string;
  /** Surface treatment actually in effect (auto resolved). */
  material: Material;
  /** True when surfaces should read as translucent material. */
  translucent: boolean;
  /** iOS 26+ Liquid Glass is available and selected. */
  liquid: boolean;
  /** A gradient or photo is painted behind the app. */
  hasBackdrop: boolean;
  backdrop: Backdrop;
  effect: WallpaperEffect;
  backdropDim: number;

  background: string;
  surface: string;
  surfaceRaised: string;
  fill: string;
  separator: string;
  text: string;
  textSecondary: string;
  textTertiary: string;
  accent: string;
  accentStrong: string;
  accentContrast: string;
  accentMuted: string;
  link: string;
  danger: string;
  warning: string;
  success: string;
  composerBg: string;
  userBubble: string;
  userBubbleText: string;
  codeBg: string;
  /** Fill for a translucent card when glass/blur is not rendering (Android < 12, Reduce Transparency). */
  glassFallback: string;
  /** Hairline highlight used on the rim of translucent cards. */
  glassStroke: string;
}

const liquidAvailable = Platform.OS === 'ios' && isLiquidGlassAvailable() && isGlassEffectAPIAvailable();

export function resolveTheme(prefs: ThemePrefs, systemScheme: Scheme, reduceTransparency: boolean): Theme {
  const scheme: Scheme = prefs.appearance === 'system' ? systemScheme : prefs.appearance;
  const fam = familyById(prefs.family);
  const own = fam[scheme];
  // Dark-only families borrow the Cosmos light palette but keep their accent.
  const base = own ?? { ...families[0]![scheme]!, accent: (fam.dark ?? fam.light)!.accent };
  const dark = scheme === 'dark';

  const [accent, accentStrong]: [string, string] =
    prefs.accent === 'theme' ? [base.accent, base.accent]
      : prefs.accent === 'mono' ? [base.text, base.text]
      : ACCENTS[prefs.accent][scheme];

  const hasBackdrop = prefs.backdrop.kind !== 'none';
  const wanted: Material = prefs.surface === 'auto' ? (liquidAvailable ? 'glass' : 'acrylic') : prefs.surface;
  const material: Material = reduceTransparency ? 'opaque' : wanted === 'glass' && !liquidAvailable ? 'acrylic' : wanted;
  const translucent = material !== 'opaque';

  const surface = base.card !== base.background ? base.card : base.shell;
  const surfaceRaised = dark ? mix(surface, base.text, 0.08) : mix(surface, '#ffffff', 0.6);

  return {
    scheme,
    familyId: fam.id,
    familyName: fam.name,
    material,
    translucent,
    liquid: material === 'glass',
    hasBackdrop,
    backdrop: prefs.backdrop.kind === 'photo' ? { ...prefs.backdrop, uri: wallpaperPath(prefs.backdrop.uri) } : prefs.backdrop,
    effect: prefs.effect,
    backdropDim: prefs.backdropDim,

    background: base.background,
    surface,
    surfaceRaised,
    fill: mix(base.background, base.text, dark ? 0.12 : 0.08),
    separator: alpha(base.text, dark ? 0.14 : 0.12),
    text: base.text,
    textSecondary: base.muted,
    textTertiary: base.faint,
    accent,
    accentStrong,
    accentContrast: onColor(accentStrong),
    accentMuted: alpha(accent, dark ? 0.22 : 0.12),
    link: prefs.accent === 'mono' ? base.accent : accent,
    danger: base.danger,
    warning: base.warning,
    success: base.success,
    composerBg: translucent ? alpha(surface, dark ? 0.55 : 0.7) : surface,
    // User turns read as inverse ink (Codex-style): ink slab on paper in
    // light, a raised grey slab in dark so it doesn't glare.
    userBubble: dark ? mix(base.background, base.text, 0.16) : base.text,
    userBubbleText: dark ? base.text : base.background,
    codeBg: mix(surface, base.text, dark ? 0.06 : 0.05),
    glassFallback: alpha(surface, dark ? 0.72 : 0.78),
    glassStroke: alpha(dark ? '#ffffff' : '#000000', dark ? 0.12 : 0.06),
  };
}

// ---------------------------------------------------------------------------
// Persistence + provider

function normalize(raw: unknown): ThemePrefs {
  if (!raw || typeof raw !== 'object') return defaultPrefs;
  const p = raw as Partial<ThemePrefs>;
  const b = p.backdrop;
  const backdrop: Backdrop =
    b?.kind === 'preset' && backdropPresets.some((x) => x.id === b.id) ? b
      // Older prefs stored an absolute path; keep only the file name.
      : b?.kind === 'photo' && typeof b.uri === 'string' ? { ...b, uri: b.uri.slice(b.uri.lastIndexOf('/') + 1) }
      : { kind: 'none' };
  return {
    appearance: p.appearance === 'light' || p.appearance === 'dark' ? p.appearance : 'system',
    family: families.some((f) => f.id === p.family) ? p.family! : 'cosmos',
    accent: accentOptions.some((a) => a.id === p.accent) ? p.accent! : 'theme',
    surface: surfaceOptions.some((o) => o.id === p.surface) ? p.surface! : 'auto',
    backdrop,
    effect: wallpaperEffects.some((e) => e.id === p.effect) ? p.effect! : 'none',
    backdropDim: typeof p.backdropDim === 'number' ? Math.min(0.9, Math.max(0, p.backdropDim)) : defaultPrefs.backdropDim,
  };
}

function applyAppearance(pref: AppearancePref) {
  Appearance.setColorScheme(pref === 'system' ? 'unspecified' : pref);
}

let loaded: Promise<ThemePrefs> | null = null;
/** Restore persisted prefs at launch; resolves once the appearance is applied. */
export function loadThemePrefs(): Promise<ThemePrefs> {
  if (!loaded) {
    loaded = (async () => {
      const stored = (await SecureStore.getItemAsync(PREFS_KEY).catch(() => null))
        ?? (await SecureStore.getItemAsync(PREV_PREFS_KEY).catch(() => null));
      let prefs: ThemePrefs;
      if (stored) {
        try { prefs = normalize(JSON.parse(stored)); } catch { prefs = defaultPrefs; }
      } else {
        const legacy = await SecureStore.getItemAsync(LEGACY_APPEARANCE_KEY).catch(() => null);
        prefs = { ...defaultPrefs, appearance: legacy === 'light' || legacy === 'dark' ? legacy : 'system' };
      }
      applyAppearance(prefs.appearance);
      return prefs;
    })();
  }
  return loaded;
}

interface ThemeContextValue {
  theme: Theme;
  prefs: ThemePrefs;
  setPrefs: (patch: Partial<ThemePrefs>) => void;
}

const fallbackTheme = resolveTheme(defaultPrefs, 'light', false);
const ThemeContext = createContext<ThemeContextValue>({ theme: fallbackTheme, prefs: defaultPrefs, setPrefs: () => {} });

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const systemScheme: Scheme = useColorScheme() === 'dark' ? 'dark' : 'light';
  const [prefs, setPrefsState] = useState<ThemePrefs>(defaultPrefs);
  const [reduceTransparency, setReduceTransparency] = useState(false);

  useEffect(() => {
    void loadThemePrefs().then(setPrefsState);
    void AccessibilityInfo.isReduceTransparencyEnabled?.().then(setReduceTransparency).catch(() => {});
    const sub = AccessibilityInfo.addEventListener('reduceTransparencyChanged', setReduceTransparency);
    return () => sub.remove();
  }, []);

  const setPrefs = useCallback((patch: Partial<ThemePrefs>) => {
    setPrefsState((prev) => {
      const next = normalize({ ...prev, ...patch });
      if (patch.appearance !== undefined) applyAppearance(next.appearance);
      loaded = Promise.resolve(next);
      void SecureStore.setItemAsync(PREFS_KEY, JSON.stringify(next)).catch(() => {});
      return next;
    });
  }, []);

  const theme = useMemo(() => resolveTheme(prefs, systemScheme, reduceTransparency), [prefs, systemScheme, reduceTransparency]);
  const value = useMemo(() => ({ theme, prefs, setPrefs }), [theme, prefs, setPrefs]);
  return React.createElement(ThemeContext.Provider, { value }, children);
}

export function useTheme(): Theme {
  return useContext(ThemeContext).theme;
}

export function useThemePrefs(): [ThemePrefs, (patch: Partial<ThemePrefs>) => void] {
  const { prefs, setPrefs } = useContext(ThemeContext);
  return [prefs, setPrefs];
}

/** Seeds of the family currently in effect, for swatches. */
export function useFamilySeeds(): Seeds {
  const t = useTheme();
  const fam = familyById(t.familyId);
  return fam[t.scheme] ?? (fam.dark ?? fam.light)!;
}

export const liquidGlassSupported = liquidAvailable;

// ---------------------------------------------------------------------------
// Scales

export const spacing = (n: number) => n * 4;

// One radius ramp: 8 on chips/controls, 10–14 on cards and sheets, 20–22
// only on the primary CTA. `pill` survives for dots and the send circle.
export const radii = {
  control: 8,
  sm: 8,
  chip: 8,
  row: 12,
  panel: 12,
  md: 12,
  lg: 14,
  xl: 14,
  bubble: 14,
  xxl: 14,
  composer: 14,
  cta: 22,
  pill: 999,
};

/** Screen edge inset — one value, used everywhere content meets the screen edge. */
export const edge = spacing(4);

// One family (SF Pro / Roboto). Titles 28–34 semibold, body 16, meta 13.
export const type = {
  largeTitle: { fontSize: 34, fontWeight: '600', letterSpacing: 0.2, lineHeight: 41 },
  title: { fontSize: 28, fontWeight: '600', lineHeight: 34 },
  title2: { fontSize: 22, fontWeight: '600', lineHeight: 28 },
  title3: { fontSize: 20, fontWeight: '600', lineHeight: 25 },
  headline: { fontSize: 17, fontWeight: '600', lineHeight: 22 },
  body: { fontSize: 16, fontWeight: '400', lineHeight: 22 },
  callout: { fontSize: 16, fontWeight: '400', lineHeight: 21 },
  subhead: { fontSize: 15, fontWeight: '400', lineHeight: 20 },
  footnote: { fontSize: 13, fontWeight: '400', lineHeight: 18 },
  caption: { fontSize: 12, fontWeight: '400', lineHeight: 16 },
  caption2: { fontSize: 11, fontWeight: '500', lineHeight: 13, letterSpacing: 0.3 },
  /** Secondary metadata under a title: harness · desktop · time. */
  meta: { fontSize: 13, fontWeight: '400', lineHeight: 18 },
  /** Session card title. */
  sessionTitle: { fontSize: 16, fontWeight: '600', lineHeight: 21 },
  composer: { fontSize: 16, fontWeight: '400', lineHeight: 22 },
  /** Transcript body. Scales with Dynamic Type. */
  transcript: { fontSize: 16, fontWeight: '400', lineHeight: 23 },
  mono: { fontSize: 13, fontFamily: Platform.select({ ios: 'Menlo', android: 'monospace', default: 'monospace' }), lineHeight: 18 },
} as const satisfies Record<string, TextStyle>;

export const shadows = {
  card: '0 1px 2px rgba(0,0,0,0.06)',
  raised: '0 6px 18px rgba(0,0,0,0.12)',
  /** Focused composer: the card lifts off the page by ~8pt of shadow. */
  lift: '0 8px 24px rgba(0,0,0,0.14)',
  overlay: '0 10px 30px rgba(0,0,0,0.22)',
} as const;

// Durations from crates/ui/src/motion.rs. `fadeIn` is shortened from the
// desktop's 500ms entrance: the same curve, under 300ms, so a new message
// doesn't lag behind the thumb.
export const motion = {
  fast: 150,
  base: 250,
  slow: 400,
  fadeIn: 280,
  fadeQuick: 150,
  collapse: 180,
  resort: 260,
  /** Tab / push shared-axis transition. */
  axis: 240,
  /** WorkingIndicator `gradient-spin-pulse` period. */
  workingPulse: 750,
} as const;

/** Desktop fade-in cubic-bezier(0.16, 1, 0.3, 1). */
export const enterBezier = [0.16, 1, 0.3, 1] as const;
/** Desktop collapse cubic-bezier(0, 0, 0.58, 1). */
export const collapseBezier = [0, 0, 0.58, 1] as const;

export const statusColor = (t: Theme, s?: string): string =>
  s === 'working' ? t.link
    : s === 'awaitingInput' ? t.warning
    : s === 'errored' ? t.danger
    : s === 'completed' || s === 'connected' ? t.success
    : t.textTertiary;

// Deterministic per-chat tint for avatars (mirrors the desktop palette).
const TINTS = ['#FF3D7F', '#FF7A1A', '#FFC21A', '#22C55E', '#14B8A6', '#3B82F6', '#8B5CF6', '#A16207'];
export const tintFor = (key: string): string => {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
  return TINTS[h % TINTS.length]!;
};
