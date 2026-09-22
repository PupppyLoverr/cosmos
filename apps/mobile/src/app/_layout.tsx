import React from 'react';
import { Platform } from 'react-native';
import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { CosmosProvider } from '../lib/client';
import { ThemeProvider, loadThemePrefs, useTheme } from '../lib/theme';

// Restore the pinned appearance before the first frame so the launch doesn't flash.
void loadThemePrefs();

function ThemedStack() {
  const t = useTheme();
  const dark = t.scheme === 'dark';
  return (
    <>
      <StatusBar style={dark ? 'light' : 'dark'} />
      <Stack
        screenOptions={{
          headerTintColor: t.text,
          headerTitleStyle: { fontWeight: '600' },
          headerLargeTitleStyle: { fontWeight: '700' },
          headerLargeTitleShadowVisible: false,
          headerShadowVisible: false,
          headerStyle: { backgroundColor: t.background },
          contentStyle: { backgroundColor: t.background },
        }}
      >
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        <Stack.Screen
          name="session/[id]"
          options={({ route }) => ({
            title: (route.params as { title?: string })?.title ?? 'Session',
            headerBackTitle: 'Sessions',
            gestureEnabled: true,
            fullScreenGestureEnabled: true,
            ...(Platform.OS === 'ios' && t.translucent
              ? { headerTransparent: true, headerBlurEffect: dark ? 'systemChromeMaterialDark' : 'systemChromeMaterial' }
              : {}),
          })}
        />
        <Stack.Screen name="new-session" options={{ presentation: 'modal', title: 'New Session' }} />
        <Stack.Screen name="sign-in" options={{ presentation: 'fullScreenModal', title: 'Sign In', headerShown: false }} />
        <Stack.Screen name="pair" options={{ presentation: 'modal', title: 'Link a Desktop' }} />
        <Stack.Screen name="watch/[chatId]" options={{ title: 'Watch Live', presentation: 'modal' }} />
      </Stack>
    </>
  );
}

export default function RootLayout() {
  return (
    <ThemeProvider>
      <CosmosProvider>
        <ThemedStack />
      </CosmosProvider>
    </ThemeProvider>
  );
}
