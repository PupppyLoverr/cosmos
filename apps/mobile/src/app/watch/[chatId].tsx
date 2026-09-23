import React, { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, Pressable, StyleSheet, Text, View } from 'react-native';
import { Image } from 'expo-image';
import { useLocalSearchParams, router } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { useCosmos, useSnapshot } from '../../lib/client';
import { radii, spacing, type, useTheme } from '../../lib/theme';
import { Button, Icon, StatusChip } from '../../components/bits';
import { CircleButton } from '../../components/chrome';
import { Screen } from '../../components/surface';

// Watch Live: the desktop frame fills the screen; controls live in a thin
// bottom inset. Polls ComputerObserve on a slow cadence (bandwidth-friendly)
// while open, plus a Stop that interrupts the run remotely.

const POLL_MS = 2_500;

export default function WatchLiveScreen() {
  const { chatId } = useLocalSearchParams<{ chatId: string }>();
  const t = useTheme();
  const insets = useSafeAreaInsets();
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
        else setError('No screenshot yet. Computer Use may not be active.');
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

  const close = () => { if (router.canGoBack()) router.back(); else router.replace('/'); };

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
  const ageLabel = paused ? 'Paused' : age === null ? 'Waiting for a frame…' : age < 2 ? 'Just now' : `${age}s ago`;
  const control = computer?.control ?? computer?.state;
  const offline = connection.status !== 'connected';

  return (
    <Screen>
      <View style={[styles.top, { paddingTop: insets.top + spacing(1) }]}>
        <CircleButton icon="xmark" label="Close" onPress={close} />
        <View style={styles.titleWrap}>
          <Text style={[type.headline, { color: t.text }]} numberOfLines={1}>{chat?.title ?? 'Watch live'}</Text>
          <Text style={[type.meta, { color: offline ? t.warning : t.textSecondary }]} numberOfLines={1}>
            {offline ? 'Desktop offline — reconnecting…' : [control, ageLabel].filter(Boolean).join(' · ')}
          </Text>
        </View>
        <View style={styles.topSpacer} />
      </View>

      <View style={[styles.canvas, { backgroundColor: t.scheme === 'dark' ? '#000000' : t.fill }]}>
        {frame ? (
          <Image source={{ uri: frame }} style={styles.frame} contentFit="contain" transition={160} accessibilityLabel="Live screenshot of the desktop" />
        ) : (
          <View style={styles.center}>
            {error ? (
              <Text style={[type.footnote, { color: t.textSecondary, textAlign: 'center' }]}>{error}</Text>
            ) : (
              <ActivityIndicator color={t.textSecondary} />
            )}
          </View>
        )}
      </View>

      <View style={[styles.controls, { paddingBottom: insets.bottom + spacing(2), borderTopColor: t.separator }]}>
        <StatusChip state={session?.status === 'awaitingInput' ? 'awaitingInput' : running ? 'working' : 'idle'} />
        <Text style={[type.footnote, { color: t.textSecondary, flex: 1 }]} numberOfLines={1}>
          {running ? (session?.status === 'awaitingInput' ? 'Waiting for your answer' : 'Controlling your desktop') : 'Nothing running'}
        </Text>
        <Pressable
          onPress={() => { void Haptics.selectionAsync(); setPaused((p) => !p); }}
          hitSlop={8}
          style={({ pressed }) => [styles.pause, { backgroundColor: t.fill, opacity: pressed ? 0.6 : 1 }]}
          accessibilityRole="button"
          accessibilityLabel={paused ? 'Resume updates' : 'Pause updates'}
        >
          <Icon name={paused ? 'play.fill' : 'pause.fill'} size={13} color={t.text} weight="bold" />
        </Pressable>
        <Button title="Stop" size="md" variant="destructive" onPress={stop} loading={stopping} disabled={!running} />
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  top: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), paddingHorizontal: spacing(3), paddingBottom: spacing(2) },
  titleWrap: { flex: 1, alignItems: 'center', minWidth: 0 },
  topSpacer: { width: 44 },
  canvas: { flex: 1 },
  frame: { flex: 1 },
  center: { flex: 1, alignItems: 'center', justifyContent: 'center', padding: spacing(8) },
  controls: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(4), paddingTop: spacing(2.5), borderTopWidth: StyleSheet.hairlineWidth,
  },
  pause: { width: 44, height: 44, borderRadius: radii.md, alignItems: 'center', justifyContent: 'center' },
});
