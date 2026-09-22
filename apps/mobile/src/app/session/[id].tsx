import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Alert, KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { FlashList, FlashListRef } from '@shopify/flash-list';
import { useLocalSearchParams, router, Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { useHeaderHeight } from 'expo-router/react-navigation';
import { desktopDisplayName, useCosmos, useSnapshot } from '../../lib/client';
import { SessionMessageEntry, TranscriptUpdate, isComputerCall, type Chat } from '../../lib/protocol';
import { applyTranscriptUpdate } from '../../lib/transcript';
import { PickedFile, ensureDecodable, fileToBase64Chunks } from '../../lib/attachments';
import { notifySessionEvent } from '../../lib/notify';
import { useFlavourWord } from '../../lib/motion';
import { radii, spacing, type, useTheme, type Theme } from '../../lib/theme';
import { MessageView } from '../../components/transcript';
import { Composer, ComposerChip } from '../../components/composer';
import { Button, EmptyState, Icon, IconButton, Notice, Pulse, SuggestionCard } from '../../components/bits';
import { HarnessMark, harnessLabel } from '../../components/harness-mark';
import { Glass, Screen } from '../../components/surface';
import { SessionBanner } from '../../components/session-banner';

/** Compact header: title with the harness + live status beneath, iMessage-style. */
function HeaderTitle({ title, chat, status, desktop, t, chatId }: { title: string; chat?: Chat; status: string; desktop?: string; t: Theme; chatId: string }) {
  const parts = [harnessLabel(chat?.config?.harness), desktop].filter(Boolean);
  const live = status === 'working' || status === 'awaitingInput';
  const flavour = useFlavourWord(chatId, status === 'working');
  const sub = live
    ? (status === 'working' ? (flavour || 'Working') : 'Needs your input')
    : (parts.join(' · ') || 'Cosmos');
  return (
    <View style={styles.headerTitle} accessibilityRole="header" accessibilityLabel={`${title}, ${sub}`}>
      <Text style={[type.headline, { color: t.text }]} numberOfLines={1}>{title}</Text>
      <View style={styles.headerSub}>
        {live ? <Pulse color={status === 'working' ? t.link : t.warning} /> : null}
        <Text style={[type.caption, { color: live ? (status === 'working' ? t.link : t.warning) : t.textTertiary }]} numberOfLines={1}>
          {sub}
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
  const [newEntryCount, setNewEntryCount] = useState(0);
  const [streamEpoch, setStreamEpoch] = useState(0);
  const [freshId, setFreshId] = useState<string | null>(null);
  const listRef = useRef<FlashListRef<SessionMessageEntry>>(null);
  const seenIds = useRef<Set<string>>(new Set());
  const atBottomRef = useRef(true);
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
  }, [chatId, connection.status, client, streamEpoch]);

  useEffect(() => {
    if (!loaded || entries.length === 0) return;
    const last = entries[entries.length - 1]!;
    const isNew = !seenIds.current.has(last.id);
    const hadHistory = seenIds.current.size > 0;
    const freshEntries = entries.filter((entry) => !seenIds.current.has(entry.id));
    for (const entry of entries) seenIds.current.add(entry.id);
    if (isNew && hadHistory) setFreshId(last.id);
    if (freshEntries.length > 0 && hadHistory && !atBottomRef.current) {
      setNewEntryCount((count) => count + freshEntries.length);
    }
  }, [entries, loaded]);

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
    Alert.alert(heading, undefined, [
      ...options.map((o) => ({ text: o.label, onPress: o.onPress, style: o.destructive ? ('destructive' as const) : ('default' as const) })),
      { text: 'Cancel', style: 'cancel' as const },
    ]);
  };

  const baseConfig = () => chat?.config ?? { harness: 'opencode' as const, sandbox: 'workspace-write' as const, modelOptions: {} };

  const openModel = async () => {
    if (!chat) return;
    const base = baseConfig();
    const models = await client.listModels(base.harness).catch(() => [] as { id: string; name?: string }[]);
    if (!models.length) { Alert.alert('No models available', 'The desktop did not report any models for this harness.'); return; }
    pickFrom('Model', models.slice(0, 12).map((m) => ({
      label: `${base.model === m.id ? '✓ ' : ''}${m.name ?? m.id}`,
      onPress: () => void client.setChatConfig(chatId, { ...base, model: m.id }),
    })));
  };

  const archiveCurrent = () => {
    if (!chat) return;
    void client.setChatArchived(chatId, !chat.archived);
    if (router.canGoBack()) router.back();
    else router.replace('/');
  };

  const sessionMenu = () => {
    if (!chat) return;
    const base = baseConfig();
    pickFrom(title, [
      {
        label: `Model${base.model ? ` · ${base.model}` : ''}`,
        onPress: () => void openModel(),
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
        onPress: archiveCurrent,
      },
    ]);
  };

  const desktopName = connection.status === 'connected' && connection.desktop ? desktopDisplayName(connection.desktop) : undefined;
  const base = baseConfig();
  const emptyBody = connection.status === 'connected'
    ? 'Ask for anything — code, files, or a Computer Use task on your desktop.'
    : 'Waiting for the desktop connection…';

  return (
    <Screen>
      <Stack.Screen
        options={{
          headerTitle: () => <HeaderTitle title={title} chat={chat} status={status} desktop={desktopName} t={t} chatId={chatId} />,
          ...(Platform.OS !== 'ios' ? {
            headerRight: () => (
              <View style={styles.headerRight}>
                {cuActive || computer ? (
                  <IconButton icon="display" label="Watch live" size={32} onPress={watch} tint={cuActive ? t.link : t.textSecondary} filled={false} />
                ) : null}
                <IconButton icon="ellipsis.circle" label="Session options" size={32} onPress={sessionMenu} tint={t.textSecondary} filled={false} />
              </View>
            ),
          } : {}),
        }}
      />
      {Platform.OS === 'ios' && chat ? (
        <Stack.Toolbar placement="right">
          {cuActive || computer ? <Stack.Toolbar.Button icon="display" accessibilityLabel="Watch live" onPress={watch} /> : null}
          <Stack.Toolbar.Menu icon="ellipsis.circle" accessibilityLabel="Session options">
            <Stack.Toolbar.MenuAction icon="cpu" onPress={() => void openModel()}>
              {`Model${base.model ? ` · ${base.model}` : ''}`}
            </Stack.Toolbar.MenuAction>
            <Stack.Toolbar.Menu title="Effort" icon="gauge.with.dots.needle.33percent">
              {(['low', 'medium', 'high'] as const).map((r) => (
                <Stack.Toolbar.MenuAction
                  key={r}
                  isOn={base.reasoning === r}
                  onPress={() => void client.setChatConfig(chatId, { ...base, reasoning: r })}
                >
                  {r[0]!.toUpperCase() + r.slice(1)}
                </Stack.Toolbar.MenuAction>
              ))}
            </Stack.Toolbar.Menu>
            <Stack.Toolbar.Menu title="Sandbox" icon="lock.shield">
              {(['read-only', 'workspace-write', 'danger-full-access'] as const).map((s) => (
                <Stack.Toolbar.MenuAction
                  key={s}
                  isOn={base.sandbox === s}
                  onPress={() => void client.setChatConfig(chatId, { ...base, sandbox: s })}
                >
                  {s}
                </Stack.Toolbar.MenuAction>
              ))}
            </Stack.Toolbar.Menu>
            {cuActive || computer ? (
              <Stack.Toolbar.MenuAction icon="display" onPress={watch}>Watch Live</Stack.Toolbar.MenuAction>
            ) : null}
            <Stack.Toolbar.MenuAction
              icon={chat.archived ? 'tray.and.arrow.up' : 'archivebox'}
              destructive={!chat.archived}
              onPress={archiveCurrent}
            >
              {chat.archived ? 'Unarchive' : 'Archive'}
            </Stack.Toolbar.MenuAction>
          </Stack.Toolbar.Menu>
        </Stack.Toolbar>
      ) : null}
      <KeyboardAvoidingView
        style={styles.fill}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
        keyboardVerticalOffset={Platform.OS === 'ios' ? headerHeight : 0}
      >
        {streamLost && connection.status === 'connected' ? (
          <View style={[styles.fill, styles.center]}>
            <Notice kind="warning" title="Session disconnected" body="The desktop closed the live transcript." action={<Button title="Reconnect" size="sm" variant="tinted" style={{ alignSelf: 'flex-start', marginTop: spacing(2) }} onPress={() => { setStreamLost(false); setStreamEpoch((n) => n + 1); }} />} />
          </View>
        ) : !loaded && entries.length === 0 && connection.status === 'connected' ? (
          <View style={[styles.fill, styles.center]}><ActivityIndicator color={t.textTertiary} /></View>
        ) : connection.status !== 'connected' && entries.length === 0 ? (
          <View style={[styles.fill, styles.center]}>
            <EmptyState
              icon="laptopcomputer"
              title={connection.status === 'connecting' || connection.status === 'reconnecting' ? 'Connecting to desktop' : 'Session disconnected'}
              body={connection.detail ?? 'Link the desktop again to load this session.'}
            >
              <Button title="Desktops" size="md" variant="tinted" style={{ marginTop: spacing(4) }} onPress={() => router.push('/(tabs)/(desktops)' as never)} />
            </EmptyState>
          </View>
        ) : entries.length === 0 ? (
          <View style={[styles.fill, styles.center]}>
            <View style={styles.emptyMark}>
              <HarnessMark harness={chat?.config?.harness} size={64} />
              </View>
            <EmptyState icon="bubble.left.fill" title={harnessLabel(chat?.config?.harness) ?? 'Cosmos'} body={emptyBody} />
            <View style={styles.firstPrompts}>
              {['Summarise what changed in this repo today', 'Open Finder and tidy my Downloads', 'Find and fix the failing test'].map((p) => (
                <SuggestionCard
                  key={p}
                  onPress={() => { void Haptics.selectionAsync(); void send(p); }}
                  icon="sparkles"
                  title={p}
                />
              ))}
            </View>
          </View>
        ) : (
          <View style={styles.fill}>
            <FlashList
              ref={listRef}
              data={entries}
              keyExtractor={(e) => e.id}
              renderItem={({ item, index }) => (
                <MessageView
                  entry={item}
                  onAnswer={onAnswer}
                  grouped={index > 0 && entries[index - 1]?.role === item.role}
                  arrive={item.id === freshId}
                />
              )}
              contentInsetAdjustmentBehavior="automatic"
              contentContainerStyle={{ paddingTop: spacing(3), paddingBottom: spacing(3) }}
              maintainVisibleContentPosition={{ autoscrollToBottomThreshold: 0.2, startRenderingFromBottom: true }}
              onScroll={(e) => {
                const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
                const nextAtBottom = contentSize.height - contentOffset.y - layoutMeasurement.height < 120;
                atBottomRef.current = nextAtBottom;
                setAtBottom(nextAtBottom);
                if (nextAtBottom) setNewEntryCount(0);
              }}
              scrollEventThrottle={64}
              keyboardDismissMode="interactive"
              keyboardShouldPersistTaps="handled"
            />
            {!atBottom ? (
              <View style={styles.jumpWrap} pointerEvents="box-none">
                <Glass radius={radii.pill} thin interactive>
                  <Pressable onPress={scrollToEnd} style={styles.jump} accessibilityRole="button" accessibilityLabel={newEntryCount ? `Jump to latest, ${newEntryCount} new` : 'Jump to latest'}>
                    <Icon name="arrow.down" size={13} color={t.text} weight="semibold" />
                    <Text style={[type.footnote, { color: t.text, fontWeight: '600' }]}>{newEntryCount ? `${newEntryCount} new` : 'Latest'}</Text>
                  </Pressable>
                </Glass>
              </View>
            ) : null}
          </View>
        )}

        {cuActive || pendingInput || status === 'errored' ? (
          <View style={styles.bannerWrap}>
            {status === 'errored' ? (
              <SessionBanner kind="error" title="The last turn failed" body="Send a new message to retry" />
            ) : pendingInput ? (
              <SessionBanner kind="input" title="Cosmos is waiting for your answer" body="Tap to jump to the question" onPress={scrollToEnd} />
            ) : (
              <SessionBanner
                kind="controlling"
                title="Controlling your desktop"
                actions={(
                  <>
                    <Button title="Watch" size="sm" variant="tinted" icon="display" onPress={watch} />
                    <Button title="Stop" size="sm" variant="secondary" onPress={stop} />
                  </>
                )}
              />
            )}
          </View>
        ) : null}

        <Composer
          running={status === 'working' || status === 'awaitingInput'}
          sending={sending}
          enabled={connection.status === 'connected'}
          attachments={attachments}
          onAddAttachment={(f) => setAttachments((a) => [...a, f])}
          onRemoveAttachment={(i) => setAttachments((a) => a.filter((_, j) => j !== i))}
          onSend={(text) => void send(text)}
          onStop={stop}
          placeholder={connection.status !== 'connected' ? 'Desktop offline' : undefined}
          chips={
            <>
              <ComposerChip
                icon="cpu"
                label={chat?.config?.model ?? 'Model'}
                accessibilityLabel={`Model, ${chat?.config?.model ?? 'default'}`}
                onPress={() => void openModel()}
              />
              {chat?.branch ? (
                <ComposerChip
                  icon="git.branch"
                  label={chat.branch}
                  accessibilityLabel={`Branch ${chat.branch}`}
                  accessibilityHint="The branch this session is on. Changing the checkout stays on the desktop."
                />
              ) : null}
            </>
          }
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
  jumpWrap: { position: 'absolute', bottom: spacing(3), left: 0, right: 0, alignItems: 'center' },
  jump: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), paddingHorizontal: spacing(3.5), minHeight: 44 },
  bannerWrap: { paddingHorizontal: spacing(3), paddingTop: spacing(2) },
  firstPrompts: { alignSelf: 'stretch', gap: spacing(2), marginTop: spacing(5), paddingHorizontal: spacing(4) },
});
