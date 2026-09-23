import React, { useEffect, useRef, useState } from 'react';
import { Platform, View } from 'react-native';
import { router, Stack, useRootNavigationState } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { GestureHandlerRootView } from 'react-native-gesture-handler';
import { CosmosProvider } from '../lib/client';
import { ThemeProvider, loadThemePrefs, motion, radii, useTheme } from '../lib/theme';
import { useReduceMotion } from '../lib/motion';
import { shouldShowOnboarding, subscribeOnboardingGate } from '../lib/onboarding-store';

// Restore the pinned appearance before the first frame so the launch doesn't flash.
void loadThemePrefs();

/** Pushes the welcome once the root navigator is mounted (and again on "Show the welcome again"). */
function OnboardingGate({ show }: { show: boolean }) {
  const nav = useRootNavigationState();
  const ready = !!nav?.key;
  const pending = useRef(show);
  const [requests, setRequests] = useState(0);
  useEffect(() => subscribeOnboardingGate(() => {
    pending.current = true;
    setRequests((n) => n + 1);
  }), []);
  useEffect(() => {
    if (!ready || !pending.current) return;
    pending.current = false;
    router.push('/onboarding');
  }, [ready, requests]);
  return null;
}

function ThemedStack() {
  const t = useTheme();
  const dark = t.scheme === 'dark';
  const reduce = useReduceMotion();
  const [booted, setBooted] = useState(false);
  const [showOnboarding, setShowOnboarding] = useState(false);

  useEffect(() => {
    let live = true;
    void shouldShowOnboarding().then((show) => {
      if (!live) return;
      setShowOnboarding(show);
      setBooted(true);
    });
    return () => { live = false; };
  }, []);

  if (!booted) {
    // Hold the launch on the theme background until we know which root to draw.
    return <View style={{ flex: 1, backgroundColor: t.background }} />;
  }

  return (
    <>
      <StatusBar style={dark ? 'light' : 'dark'} />
      <Stack
        screenOptions={{
          headerTintColor: t.text,
          headerTitleStyle: { fontWeight: '600' },
          headerLargeTitleStyle: { fontWeight: '600' },
          headerLargeTitleShadowVisible: false,
          headerShadowVisible: false,
          headerStyle: { backgroundColor: t.background },
          contentStyle: { backgroundColor: t.background },
          // Shared-axis push, 240ms. Reduce Motion falls back to a plain fade.
          animation: reduce ? 'fade' : 'default',
          animationDuration: motion.axis,
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
            presentation: 'formSheet',
            headerShown: false,
            sheetAllowedDetents: [0.6, 1],
            sheetGrabberVisible: true,
            sheetCornerRadius: radii.cta,
            contentStyle: { backgroundColor: t.background },
          }}
        />
        <Stack.Screen name="sign-in" options={{ presentation: 'fullScreenModal', title: 'Sign In', headerShown: false }} />
        <Stack.Screen
          name="pair"
          options={{
            presentation: 'formSheet',
            sheetAllowedDetents: [0.75, 1],
            sheetGrabberVisible: true,
            sheetCornerRadius: radii.cta,
            headerShown: false,
            contentStyle: { backgroundColor: 'transparent' },
          }}
        />
        <Stack.Screen name="watch/[chatId]" options={{ title: 'Watch live', presentation: 'fullScreenModal', headerShown: false }} />
        <Stack.Screen
          name="onboarding"
          options={{ presentation: 'fullScreenModal', headerShown: false, gestureEnabled: false, animation: 'fade' }}
        />
      </Stack>
      <OnboardingGate show={showOnboarding} />
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
