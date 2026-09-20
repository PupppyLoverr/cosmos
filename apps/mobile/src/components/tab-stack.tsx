import React from 'react';
import { Platform } from 'react-native';
import { Stack } from 'expo-router';
import { useTheme } from '../lib/theme';

/** Stack used inside each native tab: large titles, translucent chrome over the backdrop. */
export function TabStack() {
  const t = useTheme();
  const dark = t.scheme === 'dark';
  const ios = Platform.OS === 'ios';
  return (
    <Stack
      screenOptions={{
        headerLargeTitle: ios,
        headerTransparent: ios,
        headerBlurEffect: t.translucent ? (dark ? 'systemChromeMaterialDark' : 'systemChromeMaterial') : 'none',
        headerTintColor: t.text,
        headerTitleStyle: { fontWeight: '600' },
        headerLargeTitleStyle: { fontWeight: '700' },
        headerLargeStyle: { backgroundColor: t.hasBackdrop ? 'transparent' : t.background },
        headerStyle: ios ? undefined : { backgroundColor: t.background },
        headerLargeTitleShadowVisible: false,
        headerShadowVisible: false,
        contentStyle: { backgroundColor: t.background },
      }}
    />
  );
}
