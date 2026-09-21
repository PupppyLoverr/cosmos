import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActionSheetIOS, ActivityIndicator, Alert, KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { FlashList, FlashListRef } from '@shopify/flash-list';
import { useLocalSearchParams, router, Stack } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import { desktopDisplayName, useCosmos, useSnapshot } from '../../lib/client';
import { SessionMessageEntry, TranscriptUpdate, isComputerCall, type Chat } from '../../lib/protocol';
import { applyTranscriptUpdate } from '../../lib/transcript';
import { PickedFile, ensureDecodable, fileToBase64Chunks } from '../../lib/attachments';
import { notifySessionEvent } from '../../lib/notify';
import { alpha, radii, spacing, type, useTheme, type Theme } from '../../lib/theme';
import { MessageView } from '../../components/transcript';
import { Composer } from '../../components/composer';
import { Button, EmptyState, Icon, IconButton, Notice } from '../../components/bits';
import { HarnessMark, harnessLabel } from '../../components/harness-mark';
import { Glass, Screen } from '../../components/surface';

const STATUS_LABEL: Record<string, string> = {
  working: 'Working',
  awaitingInput: 'Needs your input',
  errored: 'Failed',
  idle: 'Idle',
};

function Pulse({ color }: { color: string }) {
  const v = useSharedValue(0);
  useEffect(() => { v.value = withRepeat(withTiming(1, { duration: 1200, easing: Easing.inOut(Easing.sin) }), -1, true); }, [v]);
  const halo = useAnimatedStyle(() => ({ opacity: 0.55 - 0.45 * v.value, transform: [{ scale: 1 + 1.2 * v.value }] }));
  return (
    <View style={styles.pulse}>
      <Animated.View style={[styles.pulseHalo, { backgroundColor: color }, halo]} />
      <View style={[styles.pulseDot, { backgroundColor: color }]} />
    </View>
  );
}

/** Compact header: title with the harness + live status beneath, iMessage-style. */
function HeaderTitle({ title, chat, status, desktop, t }: { title: string; chat?: Chat; status: string; desktop?: string; t: Theme }) {
  const parts = [harnessLabel(chat?.config?.harness), desktop].filter(Boolean);
  const live = status === 'working' || status === 'awaitingInput';
  return (
    <View style={styles.headerTitle} accessibilityRole="header" accessibilityLabel={`${title}, ${STATUS_LABEL[status] ?? status}`}>
      <Text style={[type.headline, { color: t.text }]} numberOfLines={1}>{title}</Text>
      <View style={styles.headerSub}>
        {live ? <Pulse color={status === 'working' ? t.link : t.warning} /> : null}
        <Text style={[type.caption, { color: live ? (status === 'working' ? t.link : t.warning) : t.textTertiary }]} numberOfLines={1}>
          {live ? STATUS_LABEL[status] : parts.join(' · ') || 'Cosmos'}
        </Text>
      </View>
    </View>
  );
}

export default function SessionScreen() {
  const params = useLocalSearchParams<{ id: string; title?: string }>();
  const chatId = params.id!;
  const t = useTheme();
  const client = useCosmos();
  const sessions = useSnapshot(client.sessions);
  const chats = useSnapshot(client.chats);
  const spaces = useSnapshot(client.spaces);
  const connection = useSnapshot(client.connection);
  const computer = useSnapshot(client.computer);
  const headerHeight = useHeaderHeight();

  const [entries, setEntries] = useState<SessionMessageEntry[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [streamLost, setStreamLost] = useState(false);
  const [attachments, setAttachments] = useState<PickedFile[]>([]);
  const [sending, setSending] = useState(false);
  const [atBottom, setAtBottom] = useState(true);
  const listRef = useRef<FlashListRef<SessionMessageEntry>>(null);
  const prevStatus = useRef<string>('idle');

  const chat = chats.find((c) => c.id === chatId);
  const session = sessions.find((s) => s.chatId === chatId);
  const status = session?.status ?? 'idle';
  const title = chat?.title ?? params.title ?? 'Session';
  const cuActive = useMemo(
    () => entries.some((e) => e.status === 'streaming' && e.parts.some((p) => p.kind === 'tool' && isComputerCall(p.call)))
      || (status === 'working' && entries.slice(-4).some((e) => e.parts.some((p) => p.kind === 'tool' && isComputerCall(p.call)))),
    [entries, status],
  );
  const pendingInput = useMemo(
    () => entries.some((e) => e.parts.some((p) => p.kind === 'input' && !p.resolved)),
    [entries],
  );

  useEffect(() => {
    if (!client.raw() || connection.status !== 'connected') return;
    const stream = client.raw()!.stream('WatchDocMessages', { chatId }, (item) => {
      setStreamLost(false);
      setEntries((prev) => applyTranscriptUpdate(prev, item as TranscriptUpdate));
      setLoaded(true);
    });
    const settle = setTimeout(() => setLoaded(true), 1500);
    stream.done.catch(() => setStreamLost(true));
    return () => { clearTimeout(settle); stream.cancel(); };
  }, [chatId, connection.status, client]);

  useEffect(() => {
    const prev = prevStatus.current;
    prevStatus.current = status;
    if (prev === status) return;
    if (status === 'awaitingInput') {
      void notifySessionEvent('Needs your input', title, { chatId });
    } else if (prev === 'working' && status === 'idle') {
      void notifySessionEvent('Finished', title, { chatId });
    } else if (status === 'errored') {
      void notifySessionEvent('Session failed', title, { chatId });
    }
  }, [status, title, chatId]);

  const scrollToEnd = useCallback(() => listRef.current?.scrollToEnd({ animated: true }), []);

  const send = async (text: string) => {
    if (!client.raw()) return;
    setSending(true);
    try {
      const paths: string[] = [];
      for (const f of attachments) {
        const n = await ensureDecodable(f);
        const chunks = await fileToBase64Chunks(n);
        paths.push(await client.uploadAttachment(n.name, chunks));
      }
      const prompt = paths.length
        ? `${text}\n\nAttached files (uploaded):\n${paths.map((p) => `- ${p}`).join('\n')}`
        : text;
      if (status === 'working' || status === 'awaitingInput') {
        await client.steer(chatId, prompt);
      } else {
        const space = chat?.spaceId ? spaces.find((s) => s.id === chat.spaceId) : undefined;
        const cwd = chat?.cwd ?? space?.path ?? '~';
        await client.run(chatId, {
          prompt,
          cwd,
          sandbox: chat?.config?.sandbox ?? 'workspace-write',
          harness: chat?.config?.harness,
          model: chat?.config?.model ?? null,
          reasoning: chat?.config?.reasoning ?? null,
          attachments: paths,
        });
      }
      setAttachments([]);
      scrollToEnd();
    } catch (e) {
      Alert.alert("Couldn't send", e instanceof Error ? e.message : String(e));
    } finally {
      setSending(false);
    }
  };

  const onAnswer = (requestId: string, answers: { questionId: string; labels: string[] }[]) => {
    void client.respondInput(chatId, requestId, answers).catch((e) =>
      Alert.alert("Couldn't send answer", e instanceof Error ? e.message : String(e)),
    );
  };

  const stop = () => void client.interrupt(chatId);
  const watch = () => router.push({ pathname: '/watch/[chatId]', params: { chatId } });

  const pickFrom = (heading: string, options: { label: string; onPress: () => void; destructive?: boolean }[]) => {
    if (Platform.OS === 'ios') {
      ActionSheetIOS.showActionSheetWithOptions(
        {
          title: heading,
          options: [...options.map((o) => o.label), 'Cancel'],
          cancelButtonIndex: options.length,
          destructiveButtonIndex: options.findIndex((o) => o.destructive) >= 0 ? options.findIndex((o) => o.destructive) : undefined,
        },
        (i) => options[i]?.onPress(),
      );
    } else {
      Alert.alert(heading, undefined, [
        ...options.map((o) => ({ text: o.label, onPress: o.onPress, style: o.destructive ? ('destructive' as const) : ('default' as const) })),
        { text: 'Cancel', style: 'cancel' as const },
      ]);
    }
  };

  const sessionMenu = () => {
    if (!chat) return;
    const base = chat.config ?? { harness: 'opencode' as const, sandbox: 'workspace-write' as const, modelOptions: {} };
    pickFrom(title, [
      {
        label: `Model${base.model ? ` · ${base.model}` : ''}`,
        onPress: async () => {
          const models = await client.listModels(base.harness).catch(() => [] as { id: string; name?: string }[]);
          if (!models.length) { Alert.alert('No models available', 'The desktop did not report any models for this harness.'); return; }
          pickFrom('Model', models.slice(0, 12).map((m) => ({
            label: `${base.model === m.id ? '✓ ' : ''}${m.name ?? m.id}`,
            onPress: () => void client.setChatConfig(chatId, { ...base, model: m.id }),
          })));
        },
      },
      {
        label: `Effort${base.reasoning ? ` · ${base.reasoning}` : ''}`,
        onPress: () => pickFrom('Effort', (['low', 'medium', 'high'] as const).map((r) => ({
          label: `${base.reasoning === r ? '✓ ' : ''}${r[0]!.toUpperCase()}${r.slice(1)}`,
          onPress: () => void client.setChatConfig(chatId, { ...base, reasoning: r }),
        }))),
      },
      {
        label: `Sandbox · ${base.sandbox}`,
        onPress: () => pickFrom('Sandbox', (['read-only', 'workspace-write', 'danger-full-access'] as const).map((s) => ({
          label: `${base.sandbox === s ? '✓ ' : ''}${s}`,
          onPress: () => void client.setChatConfig(chatId, { ...base, sandbox: s }),
        }))),
      },
      ...(cuActive || computer ? [{ label: 'Watch Live', onPress: watch }] : []),
      {
        label: chat.archived ? 'Unarchive' : 'Archive',
        destructive: !chat.archived,
        onPress: () => { void client.setChatArchived(chatId, !chat.archived); if (router.canGoBack()) router.back(); else router.replace('/'); },
      },
    ]);
  };

  const desktopName = connection.status === 'connected' && connection.desktop ? desktopDisplayName(connection.desktop) : undefined;
  const emptyBody = connection.status === 'connected'
    ? 'Ask for anything — code, files, or a Computer Use task on your desktop.'
    : 'Waiting for the desktop connection…';

  return (
    <Screen>
      <Stack.Screen
        options={{
          headerTitle: () => <HeaderTitle title={title} chat={chat} status={status} desktop={desktopName} t={t} />,
          headerRight: () => (
            <View style={styles.headerRight}>
              {cuActive || computer ? (
                <IconButton icon="display" label="Watch live" size={32} onPress={watch} tint={cuActive ? t.link : t.textSecondary} filled={false} />
              ) : null}
              <IconButton icon="ellipsis.circle" label="Session options" size={32} onPress={sessionMenu} tint={t.textSecondary} filled={false} />
            </View>
          ),
        }}
      />
      <KeyboardAvoidingView
        style={styles.fill}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
        keyboardVerticalOffset={Platform.OS === 'ios' ? headerHeight : 0}
      >
        {streamLost && connection.status === 'connected' ? (
          <View style={[styles.fill, styles.center]}>
            <Notice kind="warning" title="Transcript stream ended" body="The desktop closed the live transcript." action={<Button title="Reconnect" size="sm" variant="tinted" style={{ alignSelf: 'flex-start', marginTop: spacing(2) }} onPress={() => setStreamLost(false)} />} />
          </View>
        ) : !loaded && entries.length === 0 && connection.status === 'connected' ? (
          <View style={[styles.fill, styles.center]}><ActivityIndicator color={t.textTertiary} /></View>
        ) : entries.length === 0 ? (
          <View style={[styles.fill, styles.center]}>
            <View style={styles.emptyMark}>
              <HarnessMark harness={chat?.config?.harness} size={64} />
            </View>
            <EmptyState icon="bubble.left.fill" title={harnessLabel(chat?.config?.harness) ?? 'Cosmos'} body={emptyBody} />
          </View>
        ) : (
          <View style={styles.fill}>
            <FlashList
              ref={listRef}
              data={entries}
              keyExtractor={(e) => e.id}
              renderItem={({ item, index }) => (
                <MessageView entry={item} onAnswer={onAnswer} grouped={index > 0 && entries[index - 1]?.role === item.role} />
              )}
              contentInsetAdjustmentBehavior="automatic"
              contentContainerStyle={{ paddingTop: spacing(3), paddingBottom: spacing(3) }}
              maintainVisibleContentPosition={{ autoscrollToBottomThreshold: 0.2, startRenderingFromBottom: true }}
              onScroll={(e) => {
                const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
                setAtBottom(contentSize.height - contentOffset.y - layoutMeasurement.height < 120);
              }}
              scrollEventThrottle={64}
              keyboardDismissMode="interactive"
              keyboardShouldPersistTaps="handled"
            />
            {!atBottom ? (
              <View style={styles.jumpWrap} pointerEvents="box-none">
                <Glass radius={radii.pill} thin interactive>
                  <Pressable onPress={scrollToEnd} style={styles.jump} accessibilityRole="button" accessibilityLabel="Jump to latest">
                    <Icon name="arrow.down" size={13} color={t.text} weight="semibold" />
                    <Text style={[type.footnote, { color: t.text, fontWeight: '600' }]}>Latest</Text>
                  </Pressable>
                </Glass>
              </View>
            ) : null}
          </View>
        )}

        {cuActive || pendingInput || status === 'errored' ? (
          <View style={styles.bannerWrap}>
            {status === 'errored' ? (
              <Notice kind="error" title="The last turn failed" body="Check the desktop for details, or send a new message to retry." />
            ) : pendingInput ? (
              <Pressable onPress={scrollToEnd} style={[styles.banner, { backgroundColor: alpha(t.warning, 0.14) }]} accessibilityRole="button">
                <Icon name="hand.raised.fill" size={15} color={t.warning} />
                <Text style={[type.subhead, { color: t.text, flex: 1, fontWeight: '600' }]}>Cosmos is waiting for your answer</Text>
                <Icon name="chevron.down" size={11} color={t.warning} weight="semibold" />
              </Pressable>
            ) : (
              <View style={[styles.banner, { backgroundColor: alpha(t.link, 0.12) }]}>
                <Pulse color={t.link} />
                <Text style={[type.subhead, { color: t.text, flex: 1, fontWeight: '600' }]}>Controlling your desktop</Text>
                <Button title="Watch" size="sm" variant="tinted" icon="display" onPress={watch} />
                <Button title="Stop" size="sm" variant="secondary" onPress={stop} />
              </View>
            )}
          </View>
        ) : null}

        <Composer
          running={status === 'working' || status === 'awaitingInput'}
          sending={sending}
          attachments={attachments}
          onAddAttachment={(f) => setAttachments((a) => [...a, f])}
          onRemoveAttachment={(i) => setAttachments((a) => a.filter((_, j) => j !== i))}
          onSend={(text) => void send(text)}
          onStop={stop}
          placeholder={connection.status !== 'connected' ? 'Desktop offline' : undefined}
        />
      </KeyboardAvoidingView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  center: { alignItems: 'center', justifyContent: 'center', padding: spacing(6) },
  emptyMark: { marginBottom: -spacing(2) },
  headerTitle: { alignItems: Platform.OS === 'ios' ? 'center' : 'flex-start', maxWidth: 220 },
  headerSub: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), marginTop: 1 },
  headerRight: { flexDirection: 'row', alignItems: 'center', gap: spacing(1) },
  pulse: { width: 10, height: 10, alignItems: 'center', justifyContent: 'center' },
  pulseHalo: { position: 'absolute', width: 8, height: 8, borderRadius: 4 },
  pulseDot: { width: 7, height: 7, borderRadius: 3.5 },
  jumpWrap: { position: 'absolute', bottom: spacing(3), left: 0, right: 0, alignItems: 'center' },
  jump: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), paddingHorizontal: spacing(3.5), height: 34 },
  bannerWrap: { paddingHorizontal: spacing(3), paddingTop: spacing(2) },
  banner: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2.5),
    paddingLeft: spacing(3.5), paddingRight: spacing(2), paddingVertical: spacing(2),
    borderRadius: radii.lg, borderCurve: 'continuous',
  },
});
