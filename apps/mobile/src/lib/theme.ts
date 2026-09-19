// Cosmos mobile theme — native, restrained, spacious. One monochrome accent
// (ink on paper in light, paper on ink in dark); color is reserved for status.
// Appearance follows the system unless the user pins Light/Dark in Settings.

import { useCallback, useEffect, useState } from 'react';
import { Appearance, useColorScheme, type TextStyle } from 'react-native';
import * as SecureStore from 'expo-secure-store';

export type AppearancePref = 'system' | 'light' | 'dark';
const APPEARANCE_KEY = 'cosmos.appearance';

export interface Theme {
  scheme: 'light' | 'dark';
  background: string;
  surface: string;
  surfaceRaised: string;
  fill: string;
  separator: string;
  text: string;
  textSecondary: string;
  textTertiary: string;
  accent: string;
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
}

export const light: Theme = {
  scheme: 'light',
  background: '#FFFFFF',
  surface: '#F4F4F5',
  surfaceRaised: '#FFFFFF',
  fill: '#ECECEE',
  separator: 'rgba(60,60,67,0.12)',
  text: '#0D0D0F',
  textSecondary: 'rgba(60,60,67,0.68)',
  textTertiary: 'rgba(60,60,67,0.42)',
  accent: '#0D0D0F',
  accentContrast: '#FFFFFF',
  accentMuted: 'rgba(13,13,15,0.08)',
  link: '#2F6BFF',
  danger: '#D92D20',
  warning: '#B54708',
  success: '#12805C',
  composerBg: '#F4F4F5',
  userBubble: '#F0F0F2',
  userBubbleText: '#0D0D0F',
  codeBg: '#EFF0F2',
};

export const dark: Theme = {
  scheme: 'dark',
  background: '#000000',
  surface: '#141416',
  surfaceRaised: '#1C1C1F',
  fill: '#242427',
  separator: 'rgba(235,235,245,0.14)',
  text: '#F5F5F7',
  textSecondary: 'rgba(235,235,245,0.64)',
  textTertiary: 'rgba(235,235,245,0.38)',
  accent: '#F5F5F7',
  accentContrast: '#000000',
  accentMuted: 'rgba(245,245,247,0.12)',
  link: '#6E9BFF',
  danger: '#F97066',
  warning: '#FEC84B',
  success: '#32D583',
  composerBg: '#1A1A1D',
  userBubble: '#26262A',
  userBubbleText: '#F5F5F7',
  codeBg: '#1F1F23',
};

export function useTheme(): Theme {
  return useColorScheme() === 'dark' ? dark : light;
}

function applyPref(pref: AppearancePref) {
  Appearance.setColorScheme(pref === 'system' ? 'unspecified' : pref);
}

/** Restore the pinned appearance at launch; resolves once applied. */
export async function loadAppearance(): Promise<AppearancePref> {
  const stored = await SecureStore.getItemAsync(APPEARANCE_KEY).catch(() => null);
  const pref: AppearancePref = stored === 'light' || stored === 'dark' ? stored : 'system';
  applyPref(pref);
  return pref;
}

export function useAppearance(): [AppearancePref, (p: AppearancePref) => void] {
  const [pref, setPref] = useState<AppearancePref>('system');
  useEffect(() => {
    void loadAppearance().then(setPref);
  }, []);
  const update = useCallback((p: AppearancePref) => {
    setPref(p);
    applyPref(p);
    void (p === 'system'
      ? SecureStore.deleteItemAsync(APPEARANCE_KEY)
      : SecureStore.setItemAsync(APPEARANCE_KEY, p));
  }, []);
  return [pref, update];
}

export const spacing = (n: number) => n * 4;

export const radii = { sm: 8, md: 12, lg: 16, xl: 22, pill: 999 };

// Apple text ramp; colors are applied at the call site from the theme.
export const type = {
  largeTitle: { fontSize: 34, fontWeight: '700', letterSpacing: 0.2 },
  title: { fontSize: 24, fontWeight: '700' },
  title2: { fontSize: 20, fontWeight: '600' },
  headline: { fontSize: 17, fontWeight: '600' },
  body: { fontSize: 17, fontWeight: '400', lineHeight: 24 },
  callout: { fontSize: 16, fontWeight: '400' },
  subhead: { fontSize: 15, fontWeight: '400', lineHeight: 20 },
  footnote: { fontSize: 13, fontWeight: '400' },
  caption: { fontSize: 12, fontWeight: '500' },
} as const satisfies Record<string, TextStyle>;

export const statusColor = (t: Theme, s?: string): string =>
  s === 'working' ? t.link
    : s === 'awaitingInput' ? t.warning
    : s === 'errored' ? t.danger
    : s === 'completed' ? t.success
    : t.textTertiary;

// Deterministic per-chat tint for avatars (mirrors the desktop palette).
const TINTS = ['#FF3D7F', '#FF7A1A', '#FFC21A', '#22C55E', '#14B8A6', '#3B82F6', '#8B5CF6', '#A16207'];
export const tintFor = (key: string): string => {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
  return TINTS[h % TINTS.length]!;
};
