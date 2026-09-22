import React, { useEffect, useMemo, useState } from 'react';
import { Alert, Platform, Pressable, RefreshControl, StyleSheet, Text, View } from 'react-native';
import { FlashList } from '@shopify/flash-list';
import { router, Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { formatTimeAgo } from '../../../lib/activity';
import { useCosmos, useSnapshot } from '../../../lib/client';
import { Chat, Device, Session, Space } from '../../../lib/protocol';
import { edge, spacing, type, useTheme } from '../../../lib/theme';
import { Button, ConnectionBanner, EmptyState, Icon, IconButton, SkeletonRows } from '../../../components/bits';
import { Screen } from '../../../components/surface';
import { SessionRow, SessionRowIndicator } from '../../../components/session-row';

function indicator(chat: Chat, sessions: Session[]): SessionRowIndicator {
  const live = sessions.find((s) => s.chatId === chat.id);
  if (live?.status === 'working') return 'working';
  if (live?.status === 'awaitingInput') return 'awaitingInput';
  if (live?.status === 'errored') return 'errored';
  if (chat.lastMessageAt && (!chat.lastSeenAt || chat.lastMessageAt > chat.lastSeenAt)) return 'completed';
  return 'idle';
}

function projectName(chat: Chat, spaces: Space[]): string {
  const space = chat.spaceId ? spaces.find((s) => s.id === chat.spaceId) : undefined;
  const named = space?.name?.trim();
  if (named) return named;
  const path = (space?.path ?? chat.cwd ?? '').replace(/\\/g, '/');
  const leaf = path.split('/').filter(Boolean).pop();
  return leaf || 'No project';
}

function deviceName(chat: Chat, devices: Device[]): string {
  return devices.find((d) => d.id === chat.deviceId)?.name?.trim() || 'Desktop';
}

function elapsedLabel(startedAt?: string | null): string | undefined {
  if (!startedAt) return undefined;
  const elapsed = Math.max(0, Date.now() - new Date(startedAt).getTime());
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

const SUGGESTIONS = [
  { label: 'Tidy my Downloads', sub: 'Sort files into folders', icon: 'folder' },
  { label: 'Take a screenshot', sub: 'Show me what is on screen', icon: 'camera.viewfinder' },
  { label: 'Draft a note', sub: 'Open TextEdit and write it', icon: 'square.and.pencil' },
];

function renameChat(client: ReturnType<typeof useCosmos>, chat: Chat) {
  Alert.prompt?.('Rename session', undefined, (title) => {
    if (title?.trim()) void client.renameChat(chat.id, title.trim());
  }, 'plain-text', chat.title ?? '');
}

function archiveChat(client: ReturnType<typeof useCosmos>, chat: Chat) {
  void client.setChatArchived(chat.id, !chat.archived);
}

function deleteChat(client: ReturnType<typeof useCosmos>, chat: Chat) {
  Alert.alert('Delete session?', 'This removes the chat from the desktop too.', [
    { text: 'Cancel', style: 'cancel' },
    { text: 'Delete', style: 'destructive', onPress: () => void client.deleteChat(chat.id) },
  ]);
}

function chatActions(client: ReturnType<typeof useCosmos>, chat: Chat) {
  void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium);
  Alert.alert(chat.title ?? 'Session', undefined, [
    { text: 'Rename', onPress: () => renameChat(client, chat) },
    { text: chat.archived ? 'Unarchive' : 'Archive', onPress: () => archiveChat(client, chat) },
    { text: 'Delete', style: 'destructive', onPress: () => deleteChat(client, chat) },
    { text: 'Cancel', style: 'cancel' },
  ]);
}

type ListItem =
  | { kind: 'section'; key: string; title: string; count: number; needsInput?: boolean }
  | { kind: 'archive'; key: string; count: number; open: boolean }
  | {
      kind: 'chat';
      key: string;
      chat: Chat;
      ind: SessionRowIndicator;
      desktop: string;
      project: string;
      startedAt?: string | null;
    };

export default function SessionsScreen() {
  const t = useTheme();
  const client = useCosmos();
  const chats = useSnapshot(client.chats);
  const sessions = useSnapshot(client.sessions);
  const spaces = useSnapshot(client.spaces);
  const devices = useSnapshot(client.devices);
  const connection = useSnapshot(client.connection);
  const [query, setQuery] = useState('');
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [tick, setTick] = useState(0);
  const loading = connection.status === 'connecting';
  const anyWorking = sessions.some((session) => session.status === 'working');

  useEffect(() => {
    if (!anyWorking) return;
    const interval = setInterval(() => setTick((value) => value + 1), 30_000);
    return () => clearInterval(interval);
  }, [anyWorking]);

  const items = useMemo<ListItem[]>(() => {
    const q = query.trim().toLowerCase();
    const sessionByChat = new Map(sessions.map((session) => [session.chatId, session]));
    const enriched = chats.map((chat) => ({
      chat,
      desktop: deviceName(chat, devices),
      project: projectName(chat, spaces),
    }));
    const matches = (value: (typeof enriched)[number]) => (
      !q
      || (value.chat.title ?? '').toLowerCase().includes(q)
      || (value.chat.lastMessagePreview ?? '').toLowerCase().includes(q)
      || (value.chat.branch ?? '').toLowerCase().includes(q)
      || value.desktop.toLowerCase().includes(q)
      || value.project.toLowerCase().includes(q)
    );
    const live = enriched
      .filter((value) => !value.chat.archived && matches(value))
      .sort((a, b) => (b.chat.lastMessageAt ?? b.chat.createdAt).localeCompare(a.chat.lastMessageAt ?? a.chat.createdAt))
      .map((value) => {
        const session = sessionByChat.get(value.chat.id);
        return {
          ...value,
          ind: indicator(value.chat, sessions),
          startedAt: session?.status === 'working' ? session.startedAt : undefined,
        };
      });
    const archived = enriched
      .filter((value) => value.chat.archived && matches(value))
      .sort((a, b) => (b.chat.lastMessageAt ?? b.chat.createdAt).localeCompare(a.chat.lastMessageAt ?? a.chat.createdAt));

    const needsInput = live.filter((value) => value.ind === 'awaitingInput' || value.ind === 'errored');
    const working = live.filter((value) => value.ind === 'working');
    const recent = live.filter((value) => value.ind === 'completed' || value.ind === 'idle');
    const groups = [needsInput.length, working.length, recent.length, archived.length].filter(Boolean).length;
    const headed = groups > 1;
    const out: ListItem[] = [];
    const pushChats = (title: string, group: typeof live, needsInputGroup = false) => {
      if (!group.length) return;
      if (headed) out.push({ kind: 'section', key: `s:${title}`, title, count: group.length, needsInput: needsInputGroup });
      for (const value of group) {
        out.push({
          kind: 'chat',
          key: value.chat.id,
          chat: value.chat,
          ind: value.ind,
          desktop: value.desktop,
          project: value.project,
          startedAt: value.startedAt,
        });
      }
    };
    pushChats('Needs input', needsInput, true);
    pushChats('Working', working);
    pushChats('Recent', recent);
    if (archived.length) {
      const open = archiveOpen || q.length > 0;
      out.push({ kind: 'archive', key: 'archive', count: archived.length, open });
      if (open) {
        for (const value of archived) {
          out.push({
            kind: 'chat',
            key: `arch:${value.chat.id}`,
            chat: value.chat,
            ind: 'idle',
            desktop: value.desktop,
            project: value.project,
          });
        }
      }
    }
    return out;
  }, [archiveOpen, chats, devices, query, sessions, spaces]);

  const connected = connection.status === 'connected';
  const connecting = connection.status === 'connecting' || connection.status === 'reconnecting';
  const connectionError = connection.status === 'error';
  const startNew = () => { router.push('/new-session'); };
  const refresh = async () => {
    setRefreshing(true);
    try { await client.refreshRemotePresence(); } catch { /* ignore */ }
    setTimeout(() => setRefreshing(false), 600);
  };

  return (
    <Screen>
      <Stack.Screen
        options={{
          headerShown: true,
          title: 'Sessions',
          headerLargeTitle: true,
          headerSearchBarOptions: {
            placeholder: 'Search',
            autoCapitalize: 'none',
            hideNavigationBar: false,
            onChangeText: (event) => setQuery(event.nativeEvent.text),
            hideWhenScrolling: true,
          },
          headerRight: () => connected ? (
            <IconButton icon="square.and.pencil" label="New session" size={36} filled={false} onPress={startNew} />
          ) : null,
        }}
      />
      <FlashList
        data={items}
        extraData={tick}
        keyExtractor={(item) => item.key}
        getItemType={(item) => item.kind}
        contentContainerStyle={styles.list}
        contentInsetAdjustmentBehavior="automatic"
        keyboardDismissMode="on-drag"
        refreshControl={
          <RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} tintColor={t.textTertiary} />
        }
        ListHeaderComponent={
          <View style={styles.header}>
            <ConnectionBanner connection={connection} onPress={() => router.push('/(tabs)/(desktops)' as never)} />
          </View>
        }
        ListEmptyComponent={
          loading ? (
            <SkeletonRows />
          ) : connected ? (
            <EmptyState
              icon={query ? 'magnifyingglass' : 'sparkles'}
              title={query ? 'No results' : 'Do anything'}
              body={query ? `Nothing matches “${query}”.` : 'Your desktop is ready. Start a session and Cosmos will work on it.'}
            >
              {!query ? (
                <View style={styles.suggestions}>
                  {SUGGESTIONS.map((suggestion) => (
                    <Pressable
                      key={suggestion.label}
                      accessibilityRole="button"
                      accessibilityLabel={`${suggestion.label}. ${suggestion.sub}`}
                      onPress={() => router.push({ pathname: '/new-session', params: { prompt: `${suggestion.label}: ${suggestion.sub}` } })}
                      style={({ pressed }) => [styles.suggestion, { backgroundColor: pressed ? t.fill : t.surface, borderColor: t.separator }]}
                    >
                      <View style={[styles.suggestionIcon, { backgroundColor: t.accentMuted }]}>
                        <Icon name={suggestion.icon} size={16} color={t.link} />
                      </View>
                      <View style={{ flex: 1 }}>
                        <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{suggestion.label}</Text>
                        <Text style={[type.footnote, { color: t.textSecondary }]}>{suggestion.sub}</Text>
                      </View>
                      <Icon name="arrow.up.right" size={12} color={t.textTertiary} />
                    </Pressable>
                  ))}
                </View>
              ) : null}
            </EmptyState>
          ) : connectionError ? (
            <EmptyState
              icon="wifi.exclamationmark"
              title="Can't reach the desktop"
              body={connection.detail ?? 'Check that Cosmos is running and signed in on the desktop.'}
              actionLabel="Retry"
              onAction={() => void refresh()}
            >
              <Button title="Desktops" variant="tinted" onPress={() => router.push('/(tabs)/(desktops)' as never)} size="md" style={{ marginTop: spacing(2) }} />
            </EmptyState>
          ) : !connection.desktop ? (
            <EmptyState
              icon="laptopcomputer"
              title="Link your desktop"
              body="Cosmos runs on your desktop. Pair it once — same network or signed in from anywhere — and control it from here."
            >
              <Button title="Link a desktop" onPress={() => router.push('/pair')} size="md" style={{ marginTop: spacing(4) }} />
            </EmptyState>
          ) : (
            <EmptyState
              icon="laptopcomputer"
              title={connecting ? 'Connecting to desktop' : "Can't reach the desktop"}
              body={connection.detail ?? 'Check that Cosmos is running and signed in on the desktop.'}
            />
          )
        }
        renderItem={({ item }) => {
          if (item.kind === 'section') {
            return (
              <View style={styles.section}>
                <Text
                  accessibilityRole="header"
                  style={[type.footnote, { color: item.needsInput ? t.warning : t.textSecondary, fontWeight: '600', textTransform: 'none' }]}
                >
                  {item.title}
                </Text>
                <Text style={[type.footnote, { color: t.textTertiary }]}>{item.count}</Text>
              </View>
            );
          }
          if (item.kind === 'archive') {
            return (
              <Pressable
                onPress={() => setArchiveOpen((value) => !value)}
                accessibilityRole="button"
                accessibilityState={{ expanded: item.open }}
                accessibilityLabel={`Archived, ${item.count}`}
                style={styles.archive}
              >
                <Icon name={item.open ? 'chevron.down' : 'chevron.right'} size={12} color={t.textTertiary} weight="semibold" />
                <Text style={[type.footnote, { color: t.textSecondary, fontWeight: '600' }]}>Archived ({item.count})</Text>
              </Pressable>
            );
          }
          return (
            <SessionRow
              chat={item.chat}
              title={item.chat.title || item.chat.lastMessagePreview || 'Untitled session'}
              indicator={item.ind}
              timeAgo={formatTimeAgo(item.chat.lastMessageAt ?? item.chat.createdAt)}
              desktop={item.desktop}
              project={item.project}
              branch={item.chat.branch}
              elapsed={item.ind === 'working' ? elapsedLabel(item.startedAt) : undefined}
              translucent={t.hasBackdrop && t.translucent}
              onPress={() => { void client.markChatSeen(item.chat.id); }}
              onLongPress={Platform.OS !== 'ios' ? () => chatActions(client, item.chat) : undefined}
              onRename={() => renameChat(client, item.chat)}
              onArchive={() => archiveChat(client, item.chat)}
              onDelete={() => deleteChat(client, item.chat)}
            />
          );
        }}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  list: { paddingHorizontal: edge, paddingBottom: spacing(12) },
  header: { gap: spacing(3), paddingTop: spacing(1), paddingBottom: spacing(2) },
  section: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginTop: spacing(4),
    marginBottom: spacing(1),
    paddingHorizontal: spacing(2),
  },
  archive: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), minHeight: 44, marginTop: spacing(2), paddingHorizontal: spacing(2) },
  suggestions: { alignSelf: 'stretch', gap: spacing(2.5), marginTop: spacing(6) },
  suggestion: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(3.5), minHeight: 44,
    borderRadius: 8, borderWidth: StyleSheet.hairlineWidth,
  },
  suggestionIcon: { width: 30, height: 30, borderRadius: 6, alignItems: 'center', justifyContent: 'center' },
});
