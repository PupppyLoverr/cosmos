// Cosmos mobile theme — native, restrained, spacious. Semantic colors keyed
// off the system appearance; no hand-rolled glass or gradients.

import { useColorScheme } from 'react-native';

export interface Theme {
  scheme: 'light' | 'dark';
  background: string;
  surface: string;
  surfaceRaised: string;
  separator: string;
  text: string;
  textSecondary: string;
  textTertiary: string;
  accent: string;
  accentMuted: string;
  danger: string;
  warning: string;
  success: string;
  composerBg: string;
  userBubble: string;
  codeBg: string;
}

export const light: Theme = {
  scheme: 'light',
  background: '#FFFFFF',
  surface: '#F7F7F8',
  surfaceRaised: '#FFFFFF',
  separator: 'rgba(60,60,67,0.12)',
  text: '#1B1B1F',
  textSecondary: 'rgba(60,60,67,0.72)',
  textTertiary: 'rgba(60,60,67,0.45)',
  accent: '#3B5BDB',
  accentMuted: 'rgba(59,91,219,0.12)',
  danger: '#D92D20',
  warning: '#B54708',
  success: '#12805C',
  composerBg: '#F1F1F3',
  userBubble: '#3B5BDB',
  codeBg: '#EFF0F2',
};

export const dark: Theme = {
  scheme: 'dark',
  background: '#0B0B0C',
  surface: '#17171A',
  surfaceRaised: '#1F1F23',
  separator: 'rgba(235,235,245,0.14)',
  text: '#F2F2F5',
  textSecondary: 'rgba(235,235,245,0.66)',
  textTertiary: 'rgba(235,235,245,0.40)',
  accent: '#7C97F8',
  accentMuted: 'rgba(124,151,248,0.18)',
  danger: '#F97066',
  warning: '#FEC84B',
  success: '#32D583',
  composerBg: '#1B1B1F',
  userBubble: '#3B5BDB',
  codeBg: '#232327',
};

export function useTheme(): Theme {
  return useColorScheme() === 'dark' ? dark : light;
}

export const spacing = (n: number) => n * 4;

export const radii = { sm: 8, md: 12, lg: 18, xl: 24, pill: 999 };

export const statusColor = (t: Theme, s?: string): string =>
  s === 'working' ? t.accent
    : s === 'awaitingInput' ? t.warning
    : s === 'errored' ? t.danger
    : s === 'completed' ? t.success
    : t.textTertiary;
