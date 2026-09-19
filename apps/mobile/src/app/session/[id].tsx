import React, { useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Alert, StyleSheet, Text, View, Pressable } from 'react-native';
import { FlashList, FlashListRef } from '@shopify/flash-list';
import { useLocalSearchParams, router, Stack } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useCosmos, useSnapshot } from '../../lib/client';
import { SessionMessageEntry, TranscriptUpdate, MessagePart, isComputerCall } from '../../lib/protocol';
import { applyTranscriptUpdate } from '../../lib/transcript';
import { PickedFile, ensureDecodable, fileToBase64Chunks } from '../../lib/attachments';
import { notifySessionEvent } from '../../lib/notify';
import { useTheme, spacing } from '../../lib/theme';
import { MessageView } from '../../components/transcript';
import { Composer } from '../../components/composer';
import { EmptyState } from '../../components/bits';

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

  const [entries, setEntries] = useState<SessionMessageEntry[]>([]);
  const [subscribed, setSubscribed] = useState(false);
  const [attachments, setAttachments] = useState<PickedFile[]>([]);
  const [sending, setSending] = useState(false);
  const listRef = useRef<FlashListRef<SessionMessageEntry>>(null);
  const streamRef = useRef<{ cancel(): void } | null>(null);
  const prevStatus = useRef<string>('idle');

  const chat = chats.find((c) => c.id === chatId);
  const session = sessions.find((s) => s.chatId === chatId);
  const status = session?.status ?? 'idle';
  const running = status === 'working' || status === 'awaitingInput';
  const cuActive = useMemo(
    () => entries.some((e) =>
      e.status === 'streaming' && e.parts.some((p) => p.kind === 'tool' && isComputerCall(p.call)),
    ) || entries.slice(-4).some((e) => e.parts.some((p) => p.kind === 'tool' && isComputerCall(p.call))),
    [entries],
  );

  useEffect(() => {
    if (!client.raw() || connection.status !== 'connected') return;
    const stream = client.raw()!.stream('WatchDocMessages', { chatId }, (item) => {
      setEntries((prev) => applyTranscriptUpdate(prev, item as TranscriptUpdate));
    });
    streamRef.current = stream;
    setSubscribed(true);
    stream.done.catch(() => setSubscribed(false));
    return () => stream.cancel();
  }, [chatId, connection.status, client]);

  // Fire a local notification on status transitions while this screen is open.
  useEffect(() => {
    const prev = prevStatus.current;
    prevStatus.current = status;
    if (prev === status) return;
    if (status === 'awaitingInput') {
      void notifySessionEvent('Needs your input', chat?.title ?? 'A session is waiting for approval', { chatId });
    } else if (prev === 'working' && status === 'idle') {
      void notifySessionEvent('Finished', chat?.title ?? 'Session finished', { chatId });
    } else if (status === 'errored') {
      void notifySessionEvent('Session failed', chat?.title ?? 'Session error', { chatId });
    }
  }, [status, chat?.title, chatId]);

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
    } catch (e) {
      Alert.alert('Send failed', e instanceof Error ? e.message : String(e));
    } finally {
      setSending(false);
    }
  };

  const onAnswer = (requestId: string, answers: { questionId: string; labels: string[] }[]) => {
    void client.respondInput(chatId, requestId, answers).catch((e) =>
      Alert.alert('Answer failed', e instanceof Error ? e.message : String(e)),
    );
  };

  const changeConfig = () => {
    if (!chat) return;
    const opts = ['Change model', 'Change effort', 'Change sandbox', chat.archived ? 'Unarchive' : 'Archive', 'Cancel'];
    const apply = async (i: number) => {
      const base = chat.config ?? { harness: 'opencode' as const, sandbox: 'workspace-write' as const, modelOptions: {} };
      if (i === 0) {
        const models = await client.listModels(base.harness).catch(() => [] as { id: string; name?: string }[]);
        Alert.alert('Model', undefined, [
          ...models.slice(0, 12).map((m) => ({
            text: m.name ?? m.id,
            onPress: () => void client.setChatConfig(chatId, { ...base, model: m.id }),
          })),
          { text: 'Cancel', style: 'cancel' as const },
        ]);
      } else if (i === 1) {
        const levels = ['low', 'medium', 'high'] as const;
        Alert.alert('Effort', undefined, [
          ...levels.map((r) => ({
            text: base.reasoning === r ? `✓ ${r}` : r,
            onPress: () => void client.setChatConfig(chatId, { ...base, reasoning: r }),
          })),
          { text: 'Cancel', style: 'cancel' as const },
        ]);
      } else if (i === 2) {
        const levels = ['read-only', 'workspace-write', 'danger-full-access'] as const;
        Alert.alert('Sandbox', undefined, [
          ...levels.map((s) => ({
            text: base.sandbox === s ? `✓ ${s}` : s,
            onPress: () => void client.setChatConfig(chatId, { ...base, sandbox: s }),
          })),
          { text: 'Cancel', style: 'cancel' as const },
        ]);
      } else if (i === 3) {
        void client.setChatArchived(chatId, !chat.archived);
        router.back();
      }
    };
    Alert.alert(chat.title ?? 'Session', chat.cwd ?? undefined, opts.map((text, i) => ({ text, onPress: () => void apply(i), style: i === 4 ? 'cancel' as const : i === 3 ? 'destructive' as const : 'default' as const })));
  };

  return (
    <View style={[styles.screen, { backgroundColor: t.background }]}>
      <Stack.Screen
        options={{
          title: chat?.title ?? params.title ?? 'Session',
          headerRight: () => (
            <View style={{ flexDirection: 'row', gap: 14 }}>
              {cuActive || computer ? (
                <Pressable
                  onPress={() => router.push({ pathname: '/watch/[chatId]', params: { chatId } })}
                  hitSlop={8}
                  accessibilityLabel="Watch live"
                >
                  <SymbolView name="display" size={19} tintColor={cuActive ? t.accent : t.textTertiary} />
                </Pressable>
              ) : null}
              <Pressable onPress={changeConfig} hitSlop={8} accessibilityLabel="Session options">
                <SymbolView name="ellipsis.circle" size={19} tintColor={t.textTertiary} />
              </Pressable>
            </View>
          ),
        }}
      />
      {!subscribed && connection.status === 'connected' ? (
        <View style={styles.center}><ActivityIndicator color={t.accent} /></View>
      ) : entries.length === 0 ? (
        <EmptyState
          icon="text.bubble"
          title="No messages yet"
          body={connection.status === 'connected' ? 'Send a prompt to start the first turn.' : 'Waiting for the desktop connection…'}
        />
      ) : (
        <FlashList
          ref={listRef}
          data={entries}
          keyExtractor={(e) => e.id}
          renderItem={({ item }) => <MessageView entry={item} onAnswer={onAnswer} />}
          contentContainerStyle={{ paddingVertical: spacing(3) }}
          onContentSizeChange={() => listRef.current?.scrollToEnd({ animated: true })}
        />
      )}
      {sending ? <Text style={{ color: t.textTertiary, fontSize: 12, paddingHorizontal: spacing(4) }}>Sending…</Text> : null}
      <Composer
        running={status === 'working'}
        attachments={attachments}
        onAddAttachment={(f) => setAttachments((a) => [...a, f])}
        onRemoveAttachment={(i) => setAttachments((a) => a.filter((_, j) => j !== i))}
        onSend={(text) => void send(text)}
        onStop={() => void client.interrupt(chatId)}
        placeholder={status === 'awaitingInput' ? 'Waiting for input…' : undefined}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  center: { flex: 1, alignItems: 'center', justifyContent: 'center' },
});
