import React from 'react';
import { Stack } from 'expo-router';
import { useTheme } from '../../../lib/theme';

export default function TabStack() {
  const t = useTheme();
  return (
    <Stack
      screenOptions={{
        headerLargeTitle: true,
        headerTransparent: true,
        headerBlurEffect: t.scheme === 'dark' ? 'systemChromeMaterialDark' : 'systemChromeMaterial',
        headerTintColor: t.text,
        headerTitleStyle: { fontWeight: '600' },
        headerLargeTitleStyle: { fontWeight: '700' },
        headerLargeStyle: { backgroundColor: t.background },
        headerLargeTitleShadowVisible: false,
        headerShadowVisible: false,
        contentStyle: { backgroundColor: t.background },
      }}
    />
  );
}
