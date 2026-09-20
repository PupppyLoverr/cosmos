import React, { useMemo, useState } from 'react';
import { ActionSheetIOS, Alert, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { FlashList } from '@shopify/flash-list';
import { router, Stack } from 'expo-router';
import { useCosmos, useSnapshot } from '../../../lib/client';
import { Chat, Session } from '../../../lib/protocol';
import { useTheme, spacing, radii, statusColor, type } from '../../../lib/theme';
import { EmptyState, ConnectionBadge, IconButton, Tag, Chip } from '../../../components/bits';
import { HarnessMark } from '../../../components/harness-mark';
import { Glass, Screen } from '../../../components/surface';

function indicator(chat: Chat, sessions: Session[]): string {
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
  if (m < 1) return 'now';
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  if (h < 24 * 7) return d.toLocaleDateString([], { weekday: 'short' });
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

const STATUS_LABEL: Record<string, string> = {
  working: 'Working',
  awaitingInput: 'Needs input',
  errored: 'Failed',
  completed: 'New reply',
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
  if (Platform.OS === 'ios') {
    ActionSheetIOS.showActionSheetWithOptions(
      { options: buttons.map((b) => b.text), destructiveButtonIndex: 2, cancelButtonIndex: 3 },
      (i) => buttons[i]?.onPress?.(),
    );
  } else {
    Alert.alert(chat.title ?? 'Session', undefined, buttons);
  }
}

export default function SessionsScreen() {
  const t = useTheme();
  const client = useCosmos();
  const chats = useSnapshot(client.chats);
  const sessions = useSnapshot(client.sessions);
  const spaces = useSnapshot(client.spaces);
  const connection = useSnapshot(client.connection);
  const [query, setQuery] = useState('');

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    return chats
      .filter((c) => !c.archived)
      .filter((c) => !q || (c.title ?? '').toLowerCase().includes(q) || (c.lastMessagePreview ?? '').toLowerCase().includes(q))
      .sort((a, b) => (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt));
  }, [chats, query]);

  const spaceName = (chat: Chat) => spaces.find((s) => s.id === chat.spaceId)?.path?.split('/').pop();
  const connected = connection.status === 'connected';

  return (
    <Screen>
      <Stack.Screen
        options={{
          headerShown: true,
          title: 'Cosmos',
          headerLargeTitle: true,
          headerSearchBarOptions: {
            placeholder: 'Search sessions',
            onChangeText: (e) => setQuery(e.nativeEvent.text),
            hideWhenScrolling: true,
          },
          headerRight: () => (
            <IconButton icon="plus" label="New session" onPress={() => router.push('/new-session')} />
          ),
        }}
      />
      <FlashList
        data={rows}
        keyExtractor={(c) => c.id}
        contentContainerStyle={{ paddingBottom: spacing(8) }}
        contentInsetAdjustmentBehavior="automatic"
        ListHeaderComponent={
          <View style={styles.statusBar}>
            <ConnectionBadge status={connection.status} detail={connection.detail} />
            <Text style={[type.caption, { color: t.textTertiary, flex: 1 }]} numberOfLines={1}>
              {connection.desktop ? connection.desktop.name : 'No desktop linked'}
            </Text>
            {!connected ? (
              <Pressable onPress={() => router.push('/pair')} hitSlop={8}>
                <Text style={[type.caption, { color: t.link }]}>Link</Text>
              </Pressable>
            ) : null}
          </View>
        }
        ListEmptyComponent={
          connected ? (
            <EmptyState
              icon="sparkles"
              title={query ? 'No matches' : 'Start something'}
              body={query ? 'Try a different search.' : 'Your desktop is ready. Ask Cosmos to do anything on it.'}
            >
              {!query ? (
                <View style={styles.suggestions}>
                  {SUGGESTIONS.map((s) => (
                    <Chip
                      key={s.label}
                      label={s.label}
                      sub={s.sub}
                      icon={s.icon}
                      onPress={() => router.push({ pathname: '/new-session', params: { prompt: `${s.label}: ${s.sub}` } })}
                    />
                  ))}
                </View>
              ) : null}
            </EmptyState>
          ) : (
            <EmptyState
              icon="desktopcomputer"
              title={connection.status === 'disconnected' ? 'Link a desktop' : 'Connecting…'}
              body="Cosmos runs on your Mac. Link it once and control it from here."
              actionLabel="Link a desktop"
              onAction={() => router.push('/pair')}
            />
          )
        }
        renderItem={({ item: chat }) => {
          const ind = indicator(chat, sessions);
          const space = spaceName(chat);
          const title = chat.title || 'Untitled session';
          const body = (
            <>
              <HarnessMark key={`${chat.id}:${chat.config?.harness ?? 'cosmos'}`} harness={chat.config?.harness} size={40} status={ind} />
              <View style={styles.chatBody}>
                <View style={styles.chatTop}>
                  <Text style={[type.callout, styles.chatTitle, { color: t.text }]} numberOfLines={1}>{title}</Text>
                  {STATUS_LABEL[ind] ? <Tag label={STATUS_LABEL[ind]!} color={statusColor(t, ind)} /> : space ? <Tag label={space} /> : null}
                  <Text style={[type.footnote, { color: t.textTertiary, marginLeft: 'auto' }]}>{relTime(chat.lastMessageAt ?? chat.createdAt)}</Text>
                </View>
                <Text style={[type.subhead, { color: t.textSecondary }]} numberOfLines={1}>
                  {chat.lastMessagePreview || chat.cwd || 'No messages yet'}
                </Text>
              </View>
            </>
          );
          const open = () => {
            void client.markChatSeen(chat.id);
            router.push({ pathname: '/session/[id]', params: { id: chat.id, title } });
          };
          return t.hasBackdrop ? (
            <Pressable onPress={open} onLongPress={() => chatActions(client, chat)} style={({ pressed }) => [styles.chatCardWrap, { opacity: pressed ? 0.8 : 1 }]}>
              <Glass radius={radii.xl} interactive style={styles.chatCard}>{body}</Glass>
            </Pressable>
          ) : (
            <Pressable
              onPress={open}
              onLongPress={() => chatActions(client, chat)}
              style={({ pressed }) => [styles.chatRow, pressed && { backgroundColor: t.fill }]}
            >
              {body}
            </Pressable>
          );
        }}
        ItemSeparatorComponent={() => t.hasBackdrop ? null : <View style={[styles.sep, { backgroundColor: t.separator }]} />}
      />
    </Screen>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  statusBar: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    paddingHorizontal: spacing(4), paddingVertical: spacing(2),
  },
  suggestions: { alignSelf: 'stretch', gap: spacing(2), marginTop: spacing(5) },
  chatRow: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(4), paddingVertical: spacing(3),
  },
  chatCardWrap: { paddingHorizontal: spacing(4), paddingVertical: spacing(1.5) },
  chatCard: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), paddingHorizontal: spacing(4), paddingVertical: spacing(3.5) },
  chatBody: { flex: 1, minWidth: 0, gap: 2 },
  chatTop: { flexDirection: 'row', alignItems: 'center', gap: spacing(2) },
  chatTitle: { fontWeight: '600', flexShrink: 1 },
  sep: { height: StyleSheet.hairlineWidth, marginLeft: spacing(4) + 40 + spacing(3) },
  badge: { borderRadius: radii.pill },
});
