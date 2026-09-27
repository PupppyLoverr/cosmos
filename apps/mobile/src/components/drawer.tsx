// Sidebar navigation (Claude / Codex): the app has no tab bar. A menu button
// slides the whole screen right to reveal the sidebar — Sessions, Desktops,
// Pair, Settings, recent sessions — with the account and "New session" at
// the foot. Tap or drag the pushed-aside screen to close.

import React, { createContext, useCallback, useContext, useMemo, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View, useWindowDimensions } from 'react-native';
import { router, usePathname } from 'expo-router';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Animated, { Extrapolation, interpolate, runOnJS, useAnimatedStyle, useSharedValue, withTiming } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { useCosmos, useSnapshot } from '../lib/client';
import { spacing, type, useTheme } from '../lib/theme';
import { useReduceMotion } from '../lib/motion';
import { Icon } from './bits';
import { PillButton, SectionLabel } from './chrome';

interface DrawerApi { open: () => void; close: () => void }
const DrawerContext = createContext<DrawerApi>({ open: () => {}, close: () => {} });
export const useDrawer = () => useContext(DrawerContext);

const DURATION = 260;

const NAV: { label: string; icon: string; href: '/' | '/desktops' | '/pair' | '/settings' }[] = [
  { label: 'Sessions', icon: 'bubble.left.and.bubble.right', href: '/' },
  { label: 'Desktops', icon: 'desktopcomputer', href: '/desktops' },
  { label: 'Pair a desktop', icon: 'qrcode.viewfinder', href: '/pair' },
  { label: 'Settings', icon: 'gearshape', href: '/settings' },
];

function Sidebar({ width, onNavigate }: { width: number; onNavigate: (go: () => void) => void }) {
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const client = useCosmos();
  const chats = useSnapshot(client.chats);
  const auth = useSnapshot(client.auth);
  const pathname = usePathname();
  const recents = useMemo(
    () => chats
      .filter((c) => !c.archived)
      .sort((a, b) => (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt))
      .slice(0, 12),
    [chats],
  );
  const initials = (auth?.user.name || auth?.user.email || '')
    .split(/\s+/).filter(Boolean).slice(0, 2).map((p) => p[0]!.toUpperCase()).join('');

  return (
    <View style={[styles.sidebar, { width, paddingTop: insets.top + spacing(4), backgroundColor: t.surface }]}>
      <Text accessibilityRole="header" style={[styles.wordmark, { color: t.text }]}>Cosmos</Text>
      <ScrollView contentContainerStyle={styles.sidebarScroll} showsVerticalScrollIndicator={false}>
        {NAV.map((item) => {
          const active = item.href === '/' ? pathname === '/' : pathname.startsWith(item.href);
          return (
            <Pressable
              key={item.href}
              onPress={() => onNavigate(() => (item.href === '/' ? router.navigate('/') : router.push(item.href)))}
              accessibilityRole="button"
              accessibilityState={{ selected: active }}
              style={({ pressed }) => [styles.navRow, { opacity: pressed ? 0.55 : 1 }]}
            >
              <Icon name={item.icon} size={22} color={t.text} />
              <Text style={[styles.navLabel, { color: t.text, fontWeight: active ? '600' : '400' }]}>{item.label}</Text>
            </Pressable>
          );
        })}

        {recents.length ? (
          <>
            <SectionLabel>Recent</SectionLabel>
            {recents.map((c) => (
              <Pressable
                key={c.id}
                onPress={() => onNavigate(() => router.push({ pathname: '/session/[id]', params: { id: c.id, title: c.title ?? 'Session' } }))}
                accessibilityRole="button"
                style={({ pressed }) => [styles.recentRow, { opacity: pressed ? 0.55 : 1 }]}
              >
                <Text style={[type.body, { color: t.text }]} numberOfLines={1}>{c.title || c.lastMessagePreview || 'Untitled session'}</Text>
              </Pressable>
            ))}
          </>
        ) : null}
      </ScrollView>

      <View style={[styles.foot, { paddingBottom: insets.bottom + spacing(3) }]}>
        <Pressable
          onPress={() => onNavigate(() => router.push(auth ? '/settings' : '/sign-in'))}
          accessibilityRole="button"
          accessibilityLabel={auth ? 'Account' : 'Sign in'}
          style={[styles.avatar, { backgroundColor: t.surface }]}
        >
          {initials ? <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{initials}</Text> : <Icon name="person" size={18} color={t.text} />}
        </Pressable>
        <PillButton title="New session" icon="plus" onPress={() => onNavigate(() => router.push('/new-session'))} style={styles.newPill} />
      </View>
    </View>
  );
}

export function DrawerProvider({ children }: { children: React.ReactNode }) {
  const t = useTheme();
  const reduce = useReduceMotion();
  const { width } = useWindowDimensions();
  const panel = Math.min(Math.round(width * 0.8), 340);
  const progress = useSharedValue(0);
  const [isOpen, setIsOpen] = useState(false);

  const animateTo = useCallback((to: 0 | 1) => {
    if (to === 1) setIsOpen(true);
    progress.set(withTiming(to, { duration: reduce ? 0 : DURATION }, (done) => {
      if (done && to === 0) runOnJS(setIsOpen)(false);
    }));
  }, [progress, reduce]);

  const api = useMemo<DrawerApi>(() => ({
    open: () => { void Haptics.selectionAsync(); animateTo(1); },
    close: () => animateTo(0),
  }), [animateTo]);

  const onNavigate = (go: () => void) => { animateTo(0); go(); };

  // Drag the pushed-aside screen back to close.
  const drag = Gesture.Pan()
    .activeOffsetX([-10, 10])
    .onUpdate((e) => { progress.set(Math.min(1, Math.max(0, 1 + e.translationX / panel))); })
    .onEnd((e) => {
      const close = e.velocityX < -300 || progress.get() < 0.6;
      progress.set(withTiming(close ? 0 : 1, { duration: DURATION }, (done) => {
        if (done && close) runOnJS(setIsOpen)(false);
      }));
    });

  const mainStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: progress.get() * panel }],
    borderRadius: interpolate(progress.get(), [0, 0.05, 1], [0, 28, 28], Extrapolation.CLAMP),
  }));
  const sidebarStyle = useAnimatedStyle(() => ({
    opacity: interpolate(progress.get(), [0, 0.4, 1], [0, 0.6, 1]),
    transform: [{ translateX: (progress.get() - 1) * 32 }],
  }));

  return (
    <DrawerContext.Provider value={api}>
      <View style={[styles.root, { backgroundColor: t.surface }]}>
        {isOpen ? (
          <Animated.View style={[StyleSheet.absoluteFill, sidebarStyle]}>
            <Sidebar width={panel} onNavigate={onNavigate} />
          </Animated.View>
        ) : null}
        <Animated.View style={[styles.main, { backgroundColor: t.background }, isOpen ? { boxShadow: t.floatShadow } : null, mainStyle]}>
          {children}
          {isOpen ? (
            <GestureDetector gesture={drag}>
              <Pressable style={StyleSheet.absoluteFill} onPress={api.close} accessibilityRole="button" accessibilityLabel="Close menu" />
            </GestureDetector>
          ) : null}
        </Animated.View>
      </View>
    </DrawerContext.Provider>
  );
}

/** Invisible strip on the left edge of a root screen: swipe right to open the sidebar. */
export function DrawerEdge() {
  const { open } = useDrawer();
  const swipe = Gesture.Pan()
    .activeOffsetX(16)
    .failOffsetY([-12, 12])
    .onEnd((e) => { if (e.translationX > 48 || e.velocityX > 400) runOnJS(open)(); });
  return (
    <GestureDetector gesture={swipe}>
      <View style={styles.edge} />
    </GestureDetector>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
  main: { flex: 1, overflow: 'hidden', borderCurve: 'continuous' },
  sidebar: { flex: 1, paddingHorizontal: spacing(5) },
  wordmark: { fontSize: 30, fontWeight: '600', letterSpacing: -0.5, marginBottom: spacing(4) },
  sidebarScroll: { paddingBottom: spacing(6) },
  navRow: { flexDirection: 'row', alignItems: 'center', gap: spacing(4), minHeight: 48 },
  navLabel: { fontSize: 16, lineHeight: 21 },
  recentRow: { minHeight: 44, justifyContent: 'center' },
  foot: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), paddingTop: spacing(3) },
  avatar: { width: 48, height: 48, borderRadius: 24, alignItems: 'center', justifyContent: 'center' },
  newPill: { flex: 1 },
  edge: { position: 'absolute', left: 0, top: 120, bottom: 120, width: 12 },
});
