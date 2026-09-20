import React, { useMemo, useState } from 'react';
import { ActionSheetIOS, Alert, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { FlashList } from '@shopify/flash-list';
import { router, Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { useCosmos, useSnapshot } from '../../../lib/client';
import { Chat, Session } from '../../../lib/protocol';
import { useTheme, spacing, radii, statusColor, type, edge, alpha } from '../../../lib/theme';
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

function relTime(iso?: string | null): string {
  if (!iso) return '';
  const d = new Date(iso);
  const diff = Date.now() - d.getTime();
  const m = Math.floor(diff / 60000);
  if (m < 1) return 'Now';
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  if (h < 24 * 7) return d.toLocaleDateString([], { weekday: 'short' });
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

const STATUS_LABEL: Record<Indicator, string | null> = {
  working: 'Working',
  awaitingInput: 'Needs your input',
  errored: 'Failed',
  completed: 'New reply',
  idle: null,
};

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

type ListItem =
  | { kind: 'section'; key: string; title: string }
  | { kind: 'chat'; key: string; chat: Chat; ind: Indicator; first: boolean; last: boolean };

export default function SessionsScreen() {
  const t = useTheme();
  const client = useCosmos();
  const chats = useSnapshot(client.chats);
  const sessions = useSnapshot(client.sessions);
  const spaces = useSnapshot(client.spaces);
  const connection = useSnapshot(client.connection);
  const [query, setQuery] = useState('');

  const items = useMemo<ListItem[]>(() => {
    const q = query.trim().toLowerCase();
    const visible = chats
      .filter((c) => !c.archived)
      .filter((c) => !q || (c.title ?? '').toLowerCase().includes(q) || (c.lastMessagePreview ?? '').toLowerCase().includes(q))
      .sort((a, b) => (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt))
      .map((chat) => ({ chat, ind: indicator(chat, sessions) }));

    const active = visible.filter((v) => v.ind === 'working' || v.ind === 'awaitingInput' || v.ind === 'errored');
    const rest = visible.filter((v) => !active.includes(v));
    const out: ListItem[] = [];
    const push = (title: string, group: typeof visible) => {
      if (!group.length) return;
      if (active.length && rest.length) out.push({ kind: 'section', key: `s:${title}`, title });
      group.forEach((v, i) => out.push({ kind: 'chat', key: v.chat.id, chat: v.chat, ind: v.ind, first: i === 0, last: i === group.length - 1 }));
    };
    push('Active', active);
    push('Recent', rest);
    return out;
  }, [chats, sessions, query]);

  const spaceName = (chat: Chat) => spaces.find((s) => s.id === chat.spaceId)?.path?.split('/').pop();
  const connected = connection.status === 'connected';
  const connecting = connection.status === 'connecting' || connection.status === 'reconnecting';

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
              <StatusDot status={connected ? 'connected' : connecting ? 'working' : 'errored'} size={7} />
              <Text style={[type.footnote, { color: t.textSecondary, fontWeight: '500' }]} numberOfLines={1}>
                {connection.desktop?.name ?? 'No desktop linked'}
                {connection.desktop ? ` · ${connected ? 'Connected' : connecting ? 'Connecting…' : 'Offline'}` : ''}
              </Text>
              <Icon name="chevron.right" size={10} color={t.textTertiary} weight="semibold" />
            </Pressable>
            {connected && items.length ? (
              <Pressable
                onPress={() => { void Haptics.selectionAsync(); router.push('/new-session'); }}
                accessibilityRole="button"
                accessibilityLabel="New session"
                style={({ pressed }) => [styles.newRow, { backgroundColor: pressed ? t.fill : t.surface, borderColor: t.separator }]}
              >
                <Icon name="plus" size={14} color={t.textSecondary} weight="semibold" />
                <Text style={[type.subhead, { color: t.textSecondary }]}>New session</Text>
              </Pressable>
            ) : null}
            {!connected && connection.detail ? (
              <Notice kind="warning" title={connecting ? 'Reconnecting' : "Can't reach the desktop"} body={connection.detail} />
            ) : null}
          </View>
        }
        ListEmptyComponent={
          connected ? (
            <EmptyState
              icon={query ? 'magnifyingglass' : 'sparkles'}
              title={query ? 'No Results' : 'Start Something'}
              body={query ? `Nothing matches “${query}”.` : 'Your Mac is ready. Ask Cosmos to do anything on it.'}
            >
              {!query ? (
                <View style={styles.suggestions}>
                  {SUGGESTIONS.map((s) => (
                    <Pressable
                      key={s.label}
                      accessibilityRole="button"
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
              title={connecting ? 'Connecting…' : 'Link Your Mac'}
              body="Cosmos runs on your Mac. Link it once on the same network and control it from here."
            >
              {!connecting ? <Button title="Link a Desktop" onPress={() => router.push('/pair')} size="md" style={{ marginTop: spacing(4) }} /> : null}
            </EmptyState>
          )
        }
        renderItem={({ item }) => {
          if (item.kind === 'section') {
            return <Text style={[type.footnote, styles.section, { color: t.textSecondary }]}>{item.title.toUpperCase()}</Text>;
          }
          const { chat, ind, first, last } = item;
          const space = spaceName(chat);
          const title = chat.title || 'Untitled session';
          const status = STATUS_LABEL[ind];
          const open = () => {
            void client.markChatSeen(chat.id);
            router.push({ pathname: '/session/[id]', params: { id: chat.id, title } });
          };
          // Over a wallpaper each row is its own glass cell; otherwise rows form an inset-grouped list.
          const cells = t.hasBackdrop && t.translucent;
          const shape = cells
            ? { borderRadius: radii.md }
            : {
                borderTopLeftRadius: first ? radii.md : 0,
                borderTopRightRadius: first ? radii.md : 0,
                borderBottomLeftRadius: last ? radii.md : 0,
                borderBottomRightRadius: last ? radii.md : 0,
              };
          const content = (
            <>
              <HarnessMark key={`${chat.id}:${chat.config?.harness ?? 'cosmos'}`} harness={chat.config?.harness} size={28} status={ind} plain />
              <View style={styles.chatBody}>
                <View style={styles.chatTop}>
                  <Text style={[type.body, styles.chatTitle, { color: t.text }]} numberOfLines={1}>{title}</Text>
                  <Text style={[type.footnote, { color: ind === 'completed' ? t.link : t.textTertiary, fontWeight: ind === 'completed' ? '600' : '400' }]}>
                    {relTime(chat.lastMessageAt ?? chat.createdAt)}
                  </Text>
                </View>
                <View style={styles.chatBottom}>
                  {status ? (
                    <View style={styles.status}>
                      <StatusDot status={ind} size={6} />
                      <Text style={[type.subhead, { color: statusColor(t, ind), fontWeight: '500' }]}>{status}</Text>
                      <Text style={[type.subhead, { color: t.textTertiary }]}>·</Text>
                    </View>
                  ) : null}
                  <Text style={[type.subhead, { color: t.textSecondary, flex: 1 }]} numberOfLines={status ? 1 : 2}>
                    {chat.lastMessagePreview || (space ? `in ${space}` : chat.cwd) || 'No messages yet'}
                  </Text>
                </View>
              </View>
              {ind === 'completed' ? <View style={[styles.unread, { backgroundColor: t.link }]} /> : null}
            </>
          );
          const pressable = (bg: string | undefined) => (
            <Pressable
              onPress={open}
              onLongPress={() => chatActions(client, chat)}
              delayLongPress={350}
              accessibilityRole="button"
              accessibilityLabel={`${title}${status ? `, ${status}` : ''}`}
              style={({ pressed }) => [styles.chatRow, bg ? { backgroundColor: pressed ? t.fill : bg } : pressed && { backgroundColor: t.fill }, shape]}
            >
              {content}
            </Pressable>
          );
          return cells ? (
            <View style={styles.cellWrap}>
              <Glass radius={radii.md} interactive>{pressable(undefined)}</Glass>
            </View>
          ) : (
            <View>
              {pressable(t.surface)}
              {!last ? <View style={[styles.sep, { backgroundColor: t.separator }]} /> : null}
            </View>
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
    paddingLeft: spacing(3), paddingRight: spacing(2.5), height: 30, borderRadius: radii.pill,
  },
  section: { fontWeight: '500', letterSpacing: 0.3, marginTop: spacing(4), marginBottom: spacing(2), marginLeft: edge },
  cellWrap: { paddingBottom: spacing(2) },
  chatRow: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: edge, paddingVertical: spacing(2.5), minHeight: 64,
  },
  newRow: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    paddingHorizontal: edge, minHeight: 44, borderRadius: radii.md, borderWidth: StyleSheet.hairlineWidth,
  },
  chatBody: { flex: 1, minWidth: 0, gap: 3 },
  chatTop: { flexDirection: 'row', alignItems: 'baseline', gap: spacing(2) },
  chatTitle: { fontWeight: '600', flex: 1 },
  chatBottom: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5) },
  status: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5) },
  unread: { width: 8, height: 8, borderRadius: 4 },
  sep: { height: StyleSheet.hairlineWidth, marginLeft: edge + 28 + spacing(3) },
  suggestions: { alignSelf: 'stretch', gap: spacing(2.5), marginTop: spacing(6) },
  suggestion: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(3.5), paddingVertical: spacing(3),
    borderRadius: radii.md, borderWidth: StyleSheet.hairlineWidth,
  },
  suggestionIcon: { width: 30, height: 30, borderRadius: radii.sm, alignItems: 'center', justifyContent: 'center' },
});
