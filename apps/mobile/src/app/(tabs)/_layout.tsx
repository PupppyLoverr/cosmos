import React from 'react';
import { Platform } from 'react-native';
import { NativeTabs } from 'expo-router/unstable-native-tabs';
import { alpha, useTheme } from '../../lib/theme';

export default function TabsLayout() {
  const t = useTheme();
  const ios = Platform.OS === 'ios';
  return (
    <NativeTabs
      tintColor={t.link}
      // iOS 26 draws its own Liquid Glass bar; older iOS gets a system blur,
      // Android gets a Material 3 navigation bar tinted from the theme.
      minimizeBehavior="onScrollDown"
      blurEffect={ios && !t.liquid && t.translucent ? (t.scheme === 'dark' ? 'systemChromeMaterialDark' : 'systemChromeMaterial') : undefined}
      backgroundColor={ios ? (t.liquid ? undefined : t.translucent ? alpha(t.background, 0.6) : t.background) : t.surface}
      indicatorColor={ios ? undefined : t.accentMuted}
      rippleColor={ios ? undefined : t.accentMuted}
      iconColor={ios ? undefined : { default: t.textSecondary, selected: t.link }}
      labelStyle={ios ? undefined : { default: { color: t.textSecondary }, selected: { color: t.link } }}
    >
      <NativeTabs.Trigger name="(sessions)">
        <NativeTabs.Trigger.Label>Sessions</NativeTabs.Trigger.Label>
        <NativeTabs.Trigger.Icon sf="bubble.left.and.bubble.right" md="forum" />
      </NativeTabs.Trigger>
      <NativeTabs.Trigger name="(desktops)">
        <NativeTabs.Trigger.Label>Desktops</NativeTabs.Trigger.Label>
        <NativeTabs.Trigger.Icon sf="desktopcomputer" md="desktop_windows" />
      </NativeTabs.Trigger>
      <NativeTabs.Trigger name="(settings)">
        <NativeTabs.Trigger.Label>Settings</NativeTabs.Trigger.Label>
        <NativeTabs.Trigger.Icon sf="gearshape" md="settings" />
      </NativeTabs.Trigger>
    </NativeTabs>
  );
}
