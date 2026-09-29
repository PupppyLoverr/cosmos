import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Alert, FlatList, KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { useLocalSearchParams, router } from 'expo-router';
import { LinearGradient } from 'expo-linear-gradient';
import { desktopDisplayName, useCosmos, useSnapshot } from '../../lib/client';
import { describeLink } from '../../lib/connection-state';
import { SessionMessageEntry, TranscriptUpdate, isComputerCall, type Chat } from '../../lib/protocol';
import { applyTranscriptUpdate } from '../../lib/transcript';
import { PickedFile, ensureDecodable, fileToBase64Chunks } from '../../lib/attachments';
import { notifySessionEvent } from '../../lib/notify';
import { spacing, type, useTheme, type Theme } from '../../lib/theme';
import { MessageView } from '../../components/transcript';
import { Composer } from '../../components/composer';
import { Button, EmptyState, Icon, Notice, Pulse } from '../../components/bits';
import { harnessLabel } from '../../components/harness-mark';
import { Screen } from '../../components/surface';
import { CircleButton, TopBar, useDockInset } from '../../components/chrome';
import { showMenu } from '../../lib/menu';
import { SessionBanner } from '../../components/session-banner';

function entryKeyExtractor(entry: SessionMessageEntry): string {
  return entry.id;
}

/** Compact header: title with the harness + live status beneath, iMessage-style. */
function HeaderTitle({ title, chat, status, t }: { title: string; chat?: Chat; status: string; t: Theme }) {
  const live = status !== 'idle';
  const sub = status === 'working' ? 'Working'
    : status === 'awaitingInput' ? 'Needs you'
      : status === 'errored' ? 'Failed'
        : harnessLabel(chat?.config?.harness);
  const subColor = status === 'working' ? t.link : status === 'awaitingInput' ? t.warning : status === 'errored' ? t.danger : t.textTertiary;
  return (
    <View style={styles.headerTitle} accessibilityRole="header" accessibilityLabel={`${title}, ${sub}`}>
      <View style={styles.headerMain}>
        <Text style={[type.headline, { color: t.text, flexShrink: 1 }]} numberOfLines={1}>{title}</Text>
      </View>
      <View style={styles.headerSub}>
        {live ? <Pulse color={subColor} /> : null}
        <Text style={[type.meta, { color: subColor }]} numberOfLines={1}>
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

  const [entries, setEntries] = useState<SessionMessageEntry[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [streamLost, setStreamLost] = useState(false);
  const [attachments, setAttachments] = useState<PickedFile[]>([]);
  const [sending, setSending] = useState(false);
  const [atBottom, setAtBottom] = useState(true);
  const [newEntryCount, setNewEntryCount] = useState(0);
  const [streamEpoch, setStreamEpoch] = useState(0);
  const [freshId, setFreshId] = useState<string | null>(null);
  const dockInset = useDockInset();
  const listRef = useRef<FlatList<SessionMessageEntry>>(null);
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
    let live = true;
    const stream = client.raw()!.stream('WatchDocMessages', { chatId }, (item) => {
      setStreamLost(false);
      setEntries((prev) => applyTranscriptUpdate(prev, item as TranscriptUpdate));
      setLoaded(true);
    });
    const settle = setTimeout(() => setLoaded(true), 1500);
    stream.done.catch(() => { if (live) setStreamLost(true); });
    return () => {
      live = false;
      clearTimeout(settle);
      stream.cancel();
      setStreamLost(false);
    };
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

  const reversed = useMemo(() => [...entries].reverse(), [entries]);
  const scrollToEnd = useCallback(() => listRef.current?.scrollToOffset({ offset: 0, animated: true }), []);

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

  const onAnswer = useCallback((requestId: string, answers: { questionId: string; labels: string[] }[]) => {
    void client.respondInput(chatId, requestId, answers).catch((e) =>
      Alert.alert("Couldn't send answer", e instanceof Error ? e.message : String(e)),
    );
  }, [client, chatId]);

  const renderEntry = useCallback(({ item, index }: { item: SessionMessageEntry; index: number }) => (
    <MessageView
      entry={item}
      onAnswer={onAnswer}
      grouped={reversed[index + 1]?.role === item.role}
      arrive={item.id === freshId}
    />
  ), [onAnswer, reversed, freshId]);

  const stop = () => void client.interrupt(chatId);
  const watch = () => router.push({ pathname: '/watch/[chatId]', params: { chatId } });
  // Watch live is offered whenever a desktop is connected, not only mid-run.
  const canWatch = connection.status === 'connected';
  const back = () => { if (router.canGoBack()) router.back(); else router.replace('/'); };

  const baseConfig = () => chat?.config ?? { harness: 'opencode' as const, sandbox: 'workspace-write' as const, modelOptions: {} };

  const openModel = async () => {
    if (!chat) return;
    const base = baseConfig();
    const models = await client.listModels(base.harness).catch(() => [] as { id: string; name?: string }[]);
    if (!models.length) { Alert.alert('No models available', 'The desktop did not report any models for this harness.'); return; }
    showMenu('Model', models.slice(0, 12).map((m) => ({
      label: m.name ?? m.id,
      checked: base.model === m.id,
      onPress: () => void client.setChatConfig(chatId, { ...base, model: m.id }),
    })));
  };

  const archiveCurrent = () => {
    if (!chat) return;
    void client.setChatArchived(chatId, !chat.archived);
    back();
  };

  const sessionMenu = () => {
    if (!chat) return;
    const base = baseConfig();
    showMenu(title, [
      ...(canWatch ? [{ label: 'Watch live', onPress: watch }] : []),
      { label: `Model${base.model ? ` · ${base.model}` : ''}`, onPress: () => void openModel() },
      {
        label: `Effort${base.reasoning ? ` · ${base.reasoning}` : ''}`,
        onPress: () => showMenu('Effort', (['low', 'medium', 'high'] as const).map((r) => ({
          label: `${r[0]!.toUpperCase()}${r.slice(1)}`,
          checked: base.reasoning === r,
          onPress: () => void client.setChatConfig(chatId, { ...base, reasoning: r }),
        }))),
      },
      {
        label: `Access · ${ACCESS_LABEL[base.sandbox] ?? base.sandbox}`,
        onPress: () => showMenu('Access', (['read-only', 'workspace-write', 'danger-full-access'] as const).map((s) => ({
          label: ACCESS_LABEL[s]!,
          checked: base.sandbox === s,
          onPress: () => void client.setChatConfig(chatId, { ...base, sandbox: s }),
        }))),
      },
      { label: chat.archived ? 'Unarchive' : 'Archive', destructive: !chat.archived, onPress: archiveCurrent },
    ]);
  };

  const link = describeLink(connection, connection.desktop ? desktopDisplayName(connection.desktop) : 'your desktop');
  return (
    <Screen>
      <KeyboardAvoidingView style={styles.fill} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
        <TopBar
          left={<CircleButton icon="chevron.left" label="Back" onPress={back} />}
          title={<HeaderTitle title={title} chat={chat} status={status} t={t} />}
          right={<CircleButton icon="ellipsis" label="Session options" onPress={sessionMenu} />}
          center
          divider
        />

        {streamLost && connection.status === 'connected' ? (
          <View style={[styles.fill, styles.center]}>
            <Notice kind="warning" title="Session disconnected" body="The desktop closed the live transcript." action={<Button title="Reconnect" size="sm" variant="tinted" style={{ alignSelf: 'flex-start', marginTop: spacing(2) }} onPress={() => { setStreamLost(false); setStreamEpoch((n) => n + 1); }} />} />
          </View>
        ) : !loaded && entries.length === 0 && connection.status === 'connected' ? (
          <View style={[styles.fill, styles.center]}><ActivityIndicator color={t.textTertiary} /></View>
        ) : connection.status !== 'connected' && entries.length === 0 ? (
          <View style={[styles.fill, styles.center]}>
            <EmptyState
              title={link.title}
              body={link.body}
              actionLabel={link.needsPair ? 'Pair again' : 'Desktops'}
              onAction={() => router.push(link.needsPair ? '/pair' : '/desktops')}
            />
          </View>
        ) : entries.length === 0 ? (
          <View style={[styles.fill, styles.center]}>
            <Text style={[type.footnote, { color: t.textTertiary }]}>Send a message to start.</Text>
          </View>
        ) : (
          <View style={styles.fill}>
            <FlatList
              ref={listRef}
              data={reversed}
              inverted
              keyExtractor={entryKeyExtractor}
              renderItem={renderEntry}
              initialNumToRender={12}
              maxToRenderPerBatch={8}
              windowSize={11}
              contentContainerStyle={{ paddingTop: spacing(2), paddingBottom: spacing(3) }}
              maintainVisibleContentPosition={{ minIndexForVisible: 0, autoscrollToTopThreshold: 160 }}
              onScroll={(e) => {
                const nextAtBottom = e.nativeEvent.contentOffset.y < 120;
                if (nextAtBottom === atBottomRef.current) return;
                atBottomRef.current = nextAtBottom;
                setAtBottom(nextAtBottom);
                if (nextAtBottom) setNewEntryCount(0);
              }}
              scrollEventThrottle={64}
              keyboardDismissMode="interactive"
              keyboardShouldPersistTaps="handled"
            />
            <LinearGradient
              pointerEvents="none"
              colors={[t.background, 'transparent']}
              style={styles.transcriptFade}
            />
            {!atBottom ? (
              <View style={styles.jumpWrap} pointerEvents="box-none">
                <Pressable
                  onPress={scrollToEnd}
                  style={[styles.jump, { backgroundColor: t.floating, boxShadow: t.floatShadow }]}
                  accessibilityRole="button"
                  accessibilityLabel={newEntryCount ? `Jump to latest, ${newEntryCount} new` : 'Jump to latest'}
                >
                  <Icon name="arrow.down" size={14} color={t.text} weight="semibold" />
                  {newEntryCount ? <Text style={[type.footnote, { color: t.text, fontWeight: '600' }]}>{`${newEntryCount} new`}</Text> : null}
                </Pressable>
              </View>
            ) : null}
          </View>
        )}

        {cuActive || pendingInput || status === 'errored' ? (
          <View style={styles.bannerWrap}>
            {status === 'errored' ? (
              <SessionBanner kind="error" title="The last turn failed" body="Send a new message to retry" />
            ) : pendingInput ? (
              <SessionBanner kind="input" title="Waiting for your answer" body="Tap to jump to the question" onPress={scrollToEnd} />
            ) : (
              <SessionBanner
                kind="controlling"
                title="Computer use"
                actions={(
                  <>
                    <Pressable onPress={watch} hitSlop={8}><Text style={[type.footnote, { color: t.text, fontWeight: '600' }]}>Watch</Text></Pressable>
                    <Pressable onPress={stop} hitSlop={8}><Text style={[type.footnote, { color: t.danger, fontWeight: '600' }]}>Stop</Text></Pressable>
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
          bottomInset={dockInset}
          placeholder={connection.status !== 'connected'
            ? (link.tone === 'asleep' ? 'Desktop asleep — reconnects on wake' : link.label)
            : entries.length ? 'Follow up…' : 'Message'}
        />
      </KeyboardAvoidingView>
    </Screen>
  );
}

const ACCESS_LABEL: Record<string, string> = {
  'read-only': 'Read only',
  'workspace-write': 'Workspace',
  'danger-full-access': 'Full access',
};

const styles = StyleSheet.create({
  fill: { flex: 1 },
  center: { alignItems: 'center', justifyContent: 'center', padding: spacing(6) },
  question: { fontSize: 24, fontWeight: '500', lineHeight: 30, textAlign: 'center' },
  headerTitle: { alignItems: 'center', minWidth: 0 },
  headerMain: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), minWidth: 0 },
  headerSub: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), marginTop: 1 },
  transcriptFade: { position: 'absolute', top: 0, left: 0, right: 0, height: 24 },
  jumpWrap: { position: 'absolute', bottom: spacing(3), left: 0, right: 0, alignItems: 'center' },
  jump: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: spacing(1.5), minWidth: 44, paddingHorizontal: spacing(3.5), minHeight: 44, borderRadius: 999 },
  bannerWrap: { paddingHorizontal: spacing(3), paddingTop: spacing(1) },
});
