import React, { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, Pressable, StyleSheet, Text, View } from 'react-native';
import { Image } from 'expo-image';
import { useLocalSearchParams, router, Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import { useCosmos, useSnapshot } from '../../lib/client';
import { useReduceMotion } from '../../lib/motion';
import { alpha, radii, spacing, type, useTheme } from '../../lib/theme';
import { Button, Icon, IconButton } from '../../components/bits';
import { Glass, Screen } from '../../components/surface';

// Watch Live: near-live screenshots while the viewer is open — polls
// ComputerObserve on a slow cadence (bandwidth-friendly), plus a Stop that
// interrupts the run remotely.

const POLL_MS = 2_500;

function LivePill({ running }: { running: boolean }) {
  const t = useTheme();
  const reduce = useReduceMotion();
  const v = useSharedValue(1);
  useEffect(() => {
    if (reduce) {
      v.set(1);
      return;
    }
    v.set(running ? withRepeat(withTiming(0.35, { duration: 700, easing: Easing.inOut(Easing.quad) }), -1, true) : 1);
  }, [reduce, running, v]);
  const blink = useAnimatedStyle(() => ({ opacity: v.get() }));
  return (
    <View style={[styles.pill, { backgroundColor: running ? t.danger : alpha('#FFFFFF', 0.14) }]} accessibilityLabel={running ? 'Live' : 'Idle'}>
      <Animated.View style={[styles.pillDot, { backgroundColor: '#fff' }, blink]} />
      <Text maxFontSizeMultiplier={1.6} style={[type.caption2, { color: '#fff', fontWeight: '700', letterSpacing: 0.6 }]}>{running ? 'LIVE' : 'IDLE'}</Text>
    </View>
  );
}

export default function WatchLiveScreen() {
  const { chatId } = useLocalSearchParams<{ chatId: string }>();
  const t = useTheme();
  const client = useCosmos();
  const sessions = useSnapshot(client.sessions);
  const chats = useSnapshot(client.chats);
  const computer = useSnapshot(client.computer);
  const connection = useSnapshot(client.connection);

  const [frame, setFrame] = useState<string | null>(null);
  const [frameAt, setFrameAt] = useState<number>(0);
  const [now, setNow] = useState(() => Date.now());
  const [error, setError] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const [paused, setPaused] = useState(false);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);

  const session = sessions.find((s) => s.chatId === chatId);
  const chat = chats.find((c) => c.id === chatId);
  const running = session?.status === 'working' || session?.status === 'awaitingInput';

  useEffect(() => {
    if (paused) return;
    let cancelled = false;
    const tick = async () => {
      try {
        const shot = await client.computerObserve();
        if (cancelled) return;
        if (shot) { setFrame(shot); setFrameAt(Date.now()); setError(null); }
        else setError('The desktop returned no screenshot. Computer Use may not be active.');
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    };
    void tick();
    timer.current = setInterval(() => void tick(), POLL_MS);
    const clock = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      cancelled = true;
      if (timer.current) clearInterval(timer.current);
      clearInterval(clock);
    };
  }, [client, paused]);

  const stop = () => {
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium);
    Alert.alert('Stop this session?', 'Interrupts the task running on your desktop. Anything half-finished stays as it is.', [
      { text: 'Keep Running', style: 'cancel' },
      {
        text: 'Stop', style: 'destructive',
        onPress: async () => {
          setStopping(true);
          try {
            await client.interrupt(chatId!);
            void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
          } catch (e) {
            Alert.alert("Couldn't stop", e instanceof Error ? e.message : String(e));
          } finally {
            setStopping(false);
          }
        },
      },
    ]);
  };

  const age = frameAt ? Math.max(0, Math.round((now - frameAt) / 1000)) : null;
  const ageLabel = age === null ? 'Waiting for first frame…' : age < 2 ? 'Just now' : `${age}s ago`;
  const control = computer?.control ?? computer?.state;

  return (
    <Screen>
      <Stack.Screen
        options={{
          title: chat?.title ?? 'Watch Live',
          headerRight: () => <IconButton icon="xmark" label="Close" size={30} filled={false} tint={t.textSecondary} onPress={() => { if (router.canGoBack()) router.back(); else router.replace('/'); }} />,
        }}
      />

      <View style={styles.stage}>
        {frame ? (
          <Image source={{ uri: frame }} style={styles.frame} contentFit="contain" transition={160} accessibilityLabel="Live screenshot of the desktop" />
        ) : (
          <View style={styles.center}>
            {error ? (
              <>
                <Icon name="display.trianglebadge.exclamationmark" size={40} color={alpha('#FFFFFF', 0.5)} weight="regular" />
                <Text style={[type.subhead, styles.stageText]}>{error}</Text>
              </>
            ) : (
              <>
                <ActivityIndicator color="#fff" />
                <Text style={[type.footnote, styles.stageText]}>Connecting to the desktop display…</Text>
              </>
            )}
          </View>
        )}

        <View style={styles.overlayTop} pointerEvents="box-none">
          <LivePill running={!!running} />
          <View style={styles.overlayRight}>
            {control ? (
              <View style={styles.badge}>
                <Icon name="display" size={11} color="#fff" weight="semibold" />
                <Text maxFontSizeMultiplier={1.6} style={[type.caption2, { color: '#fff', fontWeight: '600' }]} numberOfLines={1}>{control}</Text>
              </View>
            ) : null}
            <View style={styles.badge}>
              <Text maxFontSizeMultiplier={1.6} style={[type.caption2, { color: alpha('#FFFFFF', 0.85), fontWeight: '600' }]} numberOfLines={1}>{ageLabel}</Text>
            </View>
          </View>
        </View>

        <Pressable
          onPress={() => { void Haptics.selectionAsync(); setPaused((p) => !p); }}
          style={styles.pause}
          accessibilityRole="button"
          accessibilityLabel={paused ? 'Resume updates' : 'Pause updates'}
        >
          <View style={styles.badge}>
            <Icon name={paused ? 'play.fill' : 'pause.fill'} size={11} color="#fff" weight="bold" />
            <Text style={[type.caption2, { color: '#fff', fontWeight: '600' }]}>{paused ? 'Resume' : 'Pause'}</Text>
          </View>
        </Pressable>
      </View>

      <Glass radius={radii.xl} style={styles.controls}>
        {connection.status !== 'connected' ? (
          <View style={styles.offline}>
            <Icon name="wifi.exclamationmark" size={14} color={t.warning} />
            <Text style={[type.footnote, { color: t.warning }]}>Desktop offline — reconnecting…</Text>
          </View>
        ) : null}
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]} numberOfLines={1}>
            {running ? (session?.status === 'awaitingInput' ? 'Waiting for your answer' : 'Cosmos is controlling your desktop') : 'Nothing running'}
          </Text>
          <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>
            {running ? 'Stop at any time — the desktop keeps its state.' : 'Frames refresh while a task is active.'}
          </Text>
        </View>
        <Button title="Stop" icon="stop.fill" variant="destructive" size="md" onPress={stop} loading={stopping} disabled={!running} />
      </Glass>
    </Screen>
  );
}

const styles = StyleSheet.create({
  stage: { flex: 1, margin: spacing(3), marginBottom: spacing(2), borderRadius: radii.xl, borderCurve: 'continuous', overflow: 'hidden', backgroundColor: '#000' },
  frame: { flex: 1 },
  center: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: spacing(3), padding: spacing(8) },
  stageText: { color: alpha('#FFFFFF', 0.7), textAlign: 'center' },
  overlayTop: { position: 'absolute', top: spacing(3), left: spacing(3), right: spacing(3), flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' },
  overlayRight: { flexDirection: 'row', gap: spacing(1.5), maxWidth: '60%' },
  pill: { flexDirection: 'row', alignItems: 'center', gap: 5, minHeight: 24, paddingHorizontal: spacing(2.5), borderRadius: radii.pill },
  pillDot: { width: 6, height: 6, borderRadius: 3 },
  badge: {
    flexDirection: 'row', alignItems: 'center', gap: 5, minHeight: 24, paddingHorizontal: spacing(2.5),
    borderRadius: radii.pill, backgroundColor: alpha('#000000', 0.55),
  },
  pause: { position: 'absolute', bottom: spacing(3), right: spacing(3) },
  controls: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), marginHorizontal: spacing(3), marginBottom: spacing(3), padding: spacing(3.5), flexWrap: 'wrap' },
  offline: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), width: '100%' },
});
