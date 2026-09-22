import React, { useEffect, useState } from 'react';
import { Platform, View } from 'react-native';
import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { GestureHandlerRootView } from 'react-native-gesture-handler';
import { CosmosProvider } from '../lib/client';
import { ThemeProvider, loadThemePrefs, useTheme } from '../lib/theme';
import { Onboarding } from '../lib/onboarding';
import { hasSeenOnboarding, markOnboardingSeen, subscribeOnboardingGate } from '../lib/onboarding-store';

// Restore the pinned appearance before the first frame so the launch doesn't flash.
void loadThemePrefs();

function ThemedStack() {
  const t = useTheme();
  const dark = t.scheme === 'dark';
  const [booted, setBooted] = useState(false);
  const [showOnboarding, setShowOnboarding] = useState(false);

  useEffect(() => {
    void hasSeenOnboarding().then((seen) => {
      setShowOnboarding(!seen);
      setBooted(true);
    });
    return subscribeOnboardingGate(() => setShowOnboarding(true));
  }, []);

  if (!booted) {
    // Hold the launch on the theme background until we know which root to draw.
    return <View style={{ flex: 1, backgroundColor: t.background }} />;
  }

  if (showOnboarding) {
    return (
      <ThemeProvider>
        <StatusBar style="light" />
        <Onboarding
          onFinish={() => { void markOnboardingSeen(); setShowOnboarding(false); }}
          onSkip={() => { void markOnboardingSeen(); setShowOnboarding(false); }}
        />
      </ThemeProvider>
    );
  }

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
        <Stack.Screen
          name="new-session"
          options={{
            presentation: 'modal',
            title: 'New Session',
            ...(Platform.OS === 'ios'
              ? { headerTransparent: true, headerBlurEffect: dark ? 'systemChromeMaterialDark' : 'systemChromeMaterial' }
              : {}),
          }}
        />
        <Stack.Screen name="sign-in" options={{ presentation: 'fullScreenModal', title: 'Sign In', headerShown: false }} />
        <Stack.Screen
          name="pair"
          options={{
            presentation: 'formSheet',
            sheetAllowedDetents: [0.75, 1],
            sheetGrabberVisible: true,
            headerShown: false,
            contentStyle: { backgroundColor: 'transparent' },
          }}
        />
        <Stack.Screen name="watch/[chatId]" options={{ title: 'Watch Live', presentation: 'modal' }} />
      </Stack>
    </>
  );
}

export default function RootLayout() {
  return (
    <GestureHandlerRootView style={{ flex: 1 }}>
      <ThemeProvider>
        <CosmosProvider>
          <ThemedStack />
        </CosmosProvider>
      </ThemeProvider>
    </GestureHandlerRootView>
  );
}
