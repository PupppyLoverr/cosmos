import React, { useMemo, useState } from 'react';
import { ActionSheetIOS, Alert, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { FlashList } from '@shopify/flash-list';
import { router, Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { formatTimeAgo } from '../../../lib/activity';
import { desktopDisplayName, useCosmos, useSnapshot } from '../../../lib/client';
import { Chat, Device, Session, Space } from '../../../lib/protocol';
import { useTheme, spacing, radii, type, edge, alpha, statusColor } from '../../../lib/theme';
import { EmptyState, Icon, StatusDot, Notice, Button } from '../../../components/bits';
import { HarnessMark } from '../../../components/harness-mark';
import { Glass, Screen } from '../../../components/surface';

type Indicator = 'working' | 'awaitingInput' | 'errored' | 'completed' | 'idle';

function indicator(chat: Chat, sessions: Session[]): Indicator {
  const live = sessions.find((s) => s.chatId === chat.id);
  if (live?.status === 'working') return 'working';
  if (live?.status === 'awaitingInput') return 'awaitingInput';
  if (live?.status === 'errored') return 'errored';
  if (chat.lastMessageAt && (!chat.lastSeenAt || chat.lastMessageAt > chat.lastSeenAt)) return 'completed';
  return 'idle';
}

const STATUS_LABEL: Record<Indicator, string | null> = {
  working: 'Working',
  awaitingInput: 'Input',
  errored: 'Failed',
  completed: 'Done',
  idle: null,
};

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

const SUGGESTIONS = [
  { label: 'Tidy my Downloads', sub: 'Sort files into folders', icon: 'folder' },
  { label: 'Take a screenshot', sub: 'Show me what is on screen', icon: 'camera.viewfinder' },
  { label: 'Draft a note', sub: 'Open TextEdit and write it', icon: 'square.and.pencil' },
];

function chatActions(client: ReturnType<typeof useCosmos>, chat: Chat) {
  const buttons: { text: string; onPress?: () => void; style?: 'cancel' | 'destructive' | 'default' }[] = [
    {
      text: 'Rename',
      onPress: () => {
        Alert.prompt?.('Rename session', undefined, (title) => {
          if (title?.trim()) void client.renameChat(chat.id, title.trim());
        }, 'plain-text', chat.title ?? '');
      },
    },
    {
      text: chat.archived ? 'Unarchive' : 'Archive',
      onPress: () => void client.setChatArchived(chat.id, !chat.archived),
    },
    {
      text: 'Delete',
      style: 'destructive',
      onPress: () =>
        Alert.alert('Delete session?', 'This removes the chat from the desktop too.', [
          { text: 'Cancel', style: 'cancel' },
          { text: 'Delete', style: 'destructive', onPress: () => void client.deleteChat(chat.id) },
        ]),
    },
    { text: 'Cancel', style: 'cancel' },
  ];
  void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium);
  if (Platform.OS === 'ios') {
    ActionSheetIOS.showActionSheetWithOptions(
      { title: chat.title ?? 'Session', options: buttons.map((b) => b.text), destructiveButtonIndex: 2, cancelButtonIndex: 3 },
      (i) => buttons[i]?.onPress?.(),
    );
  } else {
    Alert.alert(chat.title ?? 'Session', undefined, buttons);
  }
}

function StatusCorner({ ind, when, color }: { ind: Indicator; when: string; color: string }) {
  if (ind === 'idle') {
    return <Text style={[type.meta, { color: color }]}>{when}</Text>;
  }
  const label = STATUS_LABEL[ind]!;
  return (
    <View style={styles.status}>
      {ind === 'completed' ? (
        <Icon name="checkmark" size={11} color={color} weight="bold" />
      ) : (
        <StatusDot status={ind} size={6} pulse={ind === 'working'} />
      )}
      <Text style={[type.meta, { color, fontWeight: '600' }]}>{label}</Text>
    </View>
  );
}

type ListItem =
  | { kind: 'section'; key: string; title: string }
  | { kind: 'archive'; key: string; count: number; open: boolean }
  | { kind: 'chat'; key: string; chat: Chat; ind: Indicator };

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

  const items = useMemo<ListItem[]>(() => {
    const q = query.trim().toLowerCase();
    const matches = (c: Chat) => !q || (c.title ?? '').toLowerCase().includes(q) || (c.lastMessagePreview ?? '').toLowerCase().includes(q) || (c.branch ?? '').toLowerCase().includes(q);
    const live = chats
      .filter((c) => !c.archived && matches(c))
      .sort((a, b) => (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt))
      .map((chat) => ({ chat, ind: indicator(chat, sessions) }));
    const archived = chats
      .filter((c) => c.archived && matches(c))
      .sort((a, b) => (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt));

    const active = live.filter((v) => v.ind === 'working' || v.ind === 'awaitingInput' || v.ind === 'errored');
    const recent = live.filter((v) => v.ind === 'completed' || v.ind === 'idle');
    const groups = [active.length > 0, recent.length > 0, archived.length > 0].filter(Boolean).length;
    const headed = groups > 1;
    const out: ListItem[] = [];
    const pushChats = (title: string, group: typeof live) => {
      if (!group.length) return;
      if (headed) out.push({ kind: 'section', key: `s:${title}`, title });
      for (const v of group) out.push({ kind: 'chat', key: v.chat.id, chat: v.chat, ind: v.ind });
    };
    pushChats('Active', active);
    pushChats('Recent', recent);
    if (archived.length) {
      const open = archiveOpen || q.length > 0;
      out.push({ kind: 'archive', key: 'archive', count: archived.length, open });
      if (open) {
        for (const chat of archived) out.push({ kind: 'chat', key: `arch:${chat.id}`, chat, ind: 'idle' });
      }
    }
    return out;
  }, [chats, sessions, query, archiveOpen]);

  const connected = connection.status === 'connected';
  const connecting = connection.status === 'connecting' || connection.status === 'reconnecting';
  const startNew = () => { void Haptics.selectionAsync(); router.push('/new-session'); };

  return (
    <Screen>
      <Stack.Screen
        options={{
          headerShown: true,
          title: 'Sessions',
          headerLargeTitle: true,
          headerSearchBarOptions: {
            placeholder: 'Search',
            onChangeText: (e) => setQuery(e.nativeEvent.text),
            hideWhenScrolling: true,
          },
          headerRight: () => connected ? (
            <Pressable
              onPress={startNew}
              accessibilityRole="button"
              accessibilityLabel="New session"
              style={styles.newButton}
            >
              <Icon name="square.and.pencil" size={18} color={t.text} />
            </Pressable>
          ) : null,
        }}
      />
      <FlashList
        data={items}
        keyExtractor={(i) => i.key}
        getItemType={(i) => i.kind}
        contentContainerStyle={styles.list}
        contentInsetAdjustmentBehavior="automatic"
        keyboardDismissMode="on-drag"
        ListHeaderComponent={
          <View style={styles.header}>
            <Pressable
              onPress={() => router.push('/(tabs)/(desktops)' as never)}
              accessibilityRole="button"
              accessibilityLabel={connection.desktop ? `${connection.desktop.name}, ${connection.status}` : 'No desktop linked'}
              style={({ pressed }) => [styles.desktopPill, { backgroundColor: alpha(t.fill, pressed ? 1 : 0.7) }]}
            >
              <StatusDot status={connected ? 'connected' : connecting ? 'working' : 'errored'} size={7} pulse={connecting} />
              <Text style={[type.footnote, { color: t.textSecondary, fontWeight: '500' }]} numberOfLines={1}>
                {connection.desktop ? desktopDisplayName(connection.desktop) : 'No desktop linked'}
                {connection.desktop ? ` · ${connected ? 'Connected' : connecting ? 'Connecting…' : 'Offline'}` : ''}
              </Text>
              <Icon name="chevron.right" size={10} color={t.textTertiary} weight="semibold" />
            </Pressable>
            {!connected && connection.detail ? (
              <Notice kind="warning" title={connecting ? 'Connecting to desktop' : "Can't reach the desktop"} body={connection.detail} />
            ) : null}
          </View>
        }
        ListEmptyComponent={
          connected ? (
            <EmptyState
              icon={query ? 'magnifyingglass' : 'sparkles'}
              title={query ? 'No results' : 'Do anything'}
              body={query ? `Nothing matches “${query}”.` : 'Your desktop is ready. Start a session and Cosmos will work on it.'}
            >
              {!query ? (
                <View style={styles.suggestions}>
                  {SUGGESTIONS.map((s) => (
                    <Pressable
                      key={s.label}
                      accessibilityRole="button"
                      accessibilityLabel={`${s.label}. ${s.sub}`}
                      onPress={() => router.push({ pathname: '/new-session', params: { prompt: `${s.label}: ${s.sub}` } })}
                      style={({ pressed }) => [styles.suggestion, { backgroundColor: pressed ? t.fill : t.surface, borderColor: t.separator }]}
                    >
                      <View style={[styles.suggestionIcon, { backgroundColor: t.accentMuted }]}>
                        <Icon name={s.icon} size={16} color={t.link} />
                      </View>
                      <View style={{ flex: 1 }}>
                        <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{s.label}</Text>
                        <Text style={[type.footnote, { color: t.textSecondary }]}>{s.sub}</Text>
                      </View>
                      <Icon name="arrow.up.right" size={12} color={t.textTertiary} />
                    </Pressable>
                  ))}
                </View>
              ) : null}
            </EmptyState>
          ) : (
            <EmptyState
              icon="laptopcomputer"
              title={connecting ? 'Connecting to desktop' : 'Link your desktop'}
              body="Cosmos runs on your desktop. Pair it once — same network or signed in from anywhere — and control it from here."
            >
              {!connecting ? <Button title="Link a desktop" onPress={() => router.push('/pair')} size="md" style={{ marginTop: spacing(4) }} /> : null}
            </EmptyState>
          )
        }
        renderItem={({ item }) => {
          if (item.kind === 'section') {
            return <Text style={[type.meta, styles.section, { color: t.textSecondary }]} accessibilityRole="header">{item.title}</Text>;
          }
          if (item.kind === 'archive') {
            return (
              <Pressable
                onPress={() => setArchiveOpen((v) => !v)}
                accessibilityRole="button"
                accessibilityState={{ expanded: item.open }}
                accessibilityLabel={`Archived, ${item.count}`}
                style={styles.archive}
              >
                <Icon name={item.open ? 'chevron.down' : 'chevron.right'} size={12} color={t.textTertiary} weight="semibold" />
                <Text style={[type.meta, { color: t.textSecondary, fontWeight: '500' }]}>Archived ({item.count})</Text>
              </Pressable>
            );
          }
          const { chat, ind } = item;
          const title = chat.title || chat.lastMessagePreview || 'Untitled session';
          const status = STATUS_LABEL[ind];
          const when = formatTimeAgo(chat.lastMessageAt ?? chat.createdAt);
          const location = `${projectName(chat, spaces)} @ ${deviceName(chat, devices)}`;
          const open = () => {
            void client.markChatSeen(chat.id);
            router.push({ pathname: '/session/[id]', params: { id: chat.id, title } });
          };
          const cells = t.hasBackdrop && t.translucent;
          const row = (
            <Pressable
              onPress={open}
              onLongPress={() => chatActions(client, chat)}
              delayLongPress={350}
              accessibilityRole="button"
              accessibilityLabel={`${title}, ${location}${status ? `, ${status}` : when ? `, ${when}` : ''}${chat.branch ? `, ${chat.branch}` : ''}`}
              style={({ pressed }) => [
                styles.chatRow,
                { backgroundColor: pressed ? t.fill : cells ? undefined : t.surface, opacity: chat.archived ? 0.72 : 1 },
                { borderRadius: radii.row },
              ]}
            >
              <View style={styles.line}>
                <Text style={[type.meta, styles.grow, { color: t.textTertiary }]} numberOfLines={1}>{location}</Text>
                <StatusCorner ind={ind} when={when} color={ind === 'idle' ? t.textTertiary : statusColor(t, ind)} />
              </View>
              <View style={styles.line}>
                <HarnessMark key={`${chat.id}:${chat.config?.harness ?? 'cosmos'}`} harness={chat.config?.harness} size={13} />
                <Text style={[type.sessionTitle, styles.grow, { color: t.text }]} numberOfLines={1}>{title}</Text>
              </View>
              {chat.branch ? (
                <View style={styles.line}>
                  <Icon name="git.branch" size={11} color={t.textTertiary} />
                  <Text style={[type.meta, styles.grow, { color: t.textTertiary }]} numberOfLines={1}>{chat.branch}</Text>
                </View>
              ) : null}
            </Pressable>
          );
          return cells ? (
            <View style={styles.cellWrap}>
              <Glass radius={radii.row}>{row}</Glass>
            </View>
          ) : (
            <View style={styles.cellWrap}>{row}</View>
          );
        }}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  list: { paddingHorizontal: edge, paddingBottom: spacing(12) },
  header: { gap: spacing(3), paddingTop: spacing(1), paddingBottom: spacing(2) },
  desktopPill: {
    alignSelf: 'flex-start', flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    paddingLeft: spacing(3), paddingRight: spacing(2.5), minHeight: 44, borderRadius: radii.pill,
  },
  newButton: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  section: { marginTop: spacing(4), marginBottom: spacing(1), marginLeft: spacing(2) },
  archive: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), minHeight: 44, marginTop: spacing(2), paddingHorizontal: spacing(2) },
  cellWrap: { paddingBottom: spacing(0.5) },
  chatRow: { paddingHorizontal: spacing(2), paddingVertical: 6, gap: 2, minHeight: 44 },
  line: { flexDirection: 'row', alignItems: 'center', gap: spacing(2) },
  grow: { flex: 1, minWidth: 0 },
  status: { flexDirection: 'row', alignItems: 'center', gap: 4 },
  suggestions: { alignSelf: 'stretch', gap: spacing(2.5), marginTop: spacing(6) },
  suggestion: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(3.5), minHeight: 44,
    borderRadius: radii.row, borderWidth: StyleSheet.hairlineWidth,
  },
  suggestionIcon: { width: 30, height: 30, borderRadius: radii.control, alignItems: 'center', justifyContent: 'center' },
});
