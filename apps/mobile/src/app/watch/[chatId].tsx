import React, { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, Pressable, StyleSheet, Text, View } from 'react-native';
import { Image } from 'expo-image';
import { useLocalSearchParams, router } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useCosmos, useSnapshot } from '../../lib/client';
import { useTheme, spacing, radii } from '../../lib/theme';

// Watch Live: near-live screenshots while the viewer is open — polls
// ComputerObserve on a slow cadence (bandwidth-friendly), plus a big red STOP
// that interrupts the run remotely.

const POLL_MS = 2_500;

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
  const [error, setError] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);

  const session = sessions.find((s) => s.chatId === chatId);
  const chat = chats.find((c) => c.id === chatId);
  const running = session?.status === 'working' || session?.status === 'awaitingInput';

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const shot = await client.computerObserve();
        if (!cancelled && shot) {
          setFrame(shot);
          setFrameAt(Date.now());
          setError(null);
        } else if (!cancelled && !shot) {
          setError('No screenshot returned — is Computer running?');
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    };
    void tick();
    timer.current = setInterval(() => void tick(), POLL_MS);
    return () => {
      cancelled = true;
      if (timer.current) clearInterval(timer.current);
    };
  }, [client]);

  const stop = () => {
    Alert.alert('Stop this session?', 'Interrupts the running task on the desktop.', [
      { text: 'Cancel', style: 'cancel' },
      {
        text: 'Stop', style: 'destructive',
        onPress: async () => {
          setStopping(true);
          try {
            await client.interrupt(chatId!);
          } catch (e) {
            Alert.alert('Stop failed', e instanceof Error ? e.message : String(e));
          } finally {
            setStopping(false);
          }
        },
      },
    ]);
  };

  return (
    <View style={[styles.screen, { backgroundColor: t.background }]}>
      <View style={styles.meta}>
        <View style={{ flex: 1 }}>
          <Text style={{ color: t.text, fontSize: 16, fontWeight: '600' }} numberOfLines={1}>
            {chat?.title ?? 'Session'}
          </Text>
          <Text style={{ color: t.textSecondary, fontSize: 13 }}>
            {session?.status ?? 'idle'} · {frameAt ? `frame ${Math.max(0, Math.round((Date.now() - frameAt) / 1000))}s ago` : 'waiting…'}
            {computer ? ` · ${computer.control ?? computer.state ?? ''}` : ''}
          </Text>
        </View>
        <View style={[styles.liveChip, { backgroundColor: running ? t.danger : t.surface }]}>
          <View style={{ width: 6, height: 6, borderRadius: 3, backgroundColor: running ? '#fff' : t.textTertiary }} />
          <Text style={{ color: running ? '#fff' : t.textTertiary, fontSize: 11, fontWeight: '700' }}>{running ? 'LIVE' : 'IDLE'}</Text>
        </View>
      </View>

      <View style={[styles.stage, { backgroundColor: '#000' }]}>
        {frame ? (
          <Image source={{ uri: frame }} style={styles.frame} contentFit="contain" transition={120} />
        ) : (
          <View style={styles.center}>
            {error ? (
              <>
                <SymbolView name="display.trianglebadge.exclamationmark" size={36} tintColor={t.textTertiary} />
                <Text style={{ color: t.textSecondary, textAlign: 'center', paddingHorizontal: spacing(8) }}>{error}</Text>
              </>
            ) : (
              <ActivityIndicator color="#fff" />
            )}
          </View>
        )}
      </View>

      <View style={styles.controls}>
        {connection.status !== 'connected' ? (
          <Text style={{ color: t.warning, flex: 1, textAlign: 'center' }}>Disconnected — reconnecting…</Text>
        ) : null}
        <Pressable onPress={stop} disabled={stopping || !running} style={[styles.stopBtn, { backgroundColor: t.danger, opacity: running ? 1 : 0.4 }]}>
          {stopping ? <ActivityIndicator color="#fff" /> : (
            <>
              <SymbolView name="stop.fill" size={16} tintColor="#fff" />
              <Text style={{ color: '#fff', fontWeight: '700', fontSize: 16 }}>Stop session</Text>
            </>
          )}
        </Pressable>
        <Pressable onPress={() => router.back()} style={[styles.doneBtn, { borderColor: t.separator }]}>
          <Text style={{ color: t.text, fontWeight: '600' }}>Done</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  meta: { flexDirection: 'row', alignItems: 'center', padding: spacing(4), gap: spacing(3) },
  liveChip: {
    flexDirection: 'row', alignItems: 'center', gap: 5,
    borderRadius: radii.pill, paddingHorizontal: spacing(2.5), paddingVertical: spacing(1.25),
  },
  stage: { flex: 1, margin: spacing(3), borderRadius: radii.lg, overflow: 'hidden' },
  frame: { flex: 1 },
  center: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: spacing(3) },
  controls: { padding: spacing(4), gap: spacing(2.5) },
  stopBtn: {
    flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: spacing(2),
    borderRadius: radii.xl, paddingVertical: spacing(3.5),
  },
  doneBtn: { alignItems: 'center', paddingVertical: spacing(3), borderRadius: radii.xl, borderWidth: 1 },
});
