import React from 'react';
import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { CosmosProvider, useCosmos, useSnapshot } from '../lib/client';
import { loadAppearance, useTheme } from '../lib/theme';

// Restore a pinned Light/Dark before the first frame so the launch doesn't flash.
void loadAppearance();

function ThemedStack() {
  const t = useTheme();
  const client = useCosmos();
  const connection = useSnapshot(client.connection);
  return (
    <>
      <StatusBar style={t.scheme === 'dark' ? 'light' : 'dark'} />
      <Stack
        screenOptions={{
          headerStyle: { backgroundColor: t.background },
          headerTintColor: t.text,
          headerTitleStyle: { fontWeight: '600' },
          headerLargeTitleStyle: { fontWeight: '700' },
          headerLargeTitleShadowVisible: false,
          contentStyle: { backgroundColor: t.background },
          headerShadowVisible: false,
        }}
      >
        <Stack.Screen name="(tabs)" options={{ headerShown: false }} />
        <Stack.Screen
          name="session/[id]"
          options={({ route }) => ({
            title: (route.params as { title?: string })?.title ?? 'Session',
            headerBackTitle: 'Back',
          })}
        />
        <Stack.Screen name="new-session" options={{ presentation: 'formSheet', sheetGrabberVisible: true, sheetAllowedDetents: [0.92], title: 'New Session' }} />
        <Stack.Screen name="sign-in" options={{ presentation: 'fullScreenModal', title: 'Sign In', headerShown: false }} />
        <Stack.Screen name="pair" options={{ presentation: 'formSheet', sheetGrabberVisible: true, sheetAllowedDetents: [0.92], title: 'Link a Desktop' }} />
        <Stack.Screen
          name="watch/[chatId]"
          options={{ title: 'Watch Live', presentation: 'modal', headerTintColor: t.text }}
        />
      </Stack>
      {connection.status !== 'connected' ? null : null}
    </>
  );
}

export default function RootLayout() {
  return (
    <CosmosProvider>
      <ThemedStack />
    </CosmosProvider>
  );
}
