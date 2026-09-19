import React, { useMemo } from 'react';
import { ActionSheetIOS, Alert, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { FlashList } from '@shopify/flash-list';
import { router, Stack } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useCosmos, useSnapshot } from '../../lib/client';
import { Chat, Session } from '../../lib/protocol';
import { useTheme, spacing, radii, statusColor } from '../../lib/theme';
import { EmptyState, ConnectionBadge } from '../../components/bits';

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
  const diff = Date.now() - new Date(iso).getTime();
  const m = Math.floor(diff / 60000);
  if (m < 1) return 'now';
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

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
        Alert.alert('Delete session?', 'This tombstones the chat on the desktop.', [
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

  const rows = useMemo(() => {
    return chats
      .filter((c) => !c.archived)
      .sort((a, b) => (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt));
  }, [chats]);

  const spaceName = (chat: Chat) => spaces.find((s) => s.id === chat.spaceId)?.path?.split('/').pop();

  return (
    <View style={[styles.screen, { backgroundColor: t.background }]}>
      <Stack.Screen
        options={{
          headerShown: true,
          title: 'Cosmos',
          headerLargeTitle: true,
          headerRight: () => (
            <Pressable onPress={() => router.push('/new-session')} hitSlop={12} accessibilityLabel="New session">
              <SymbolView name="square.and.pencil" size={20} tintColor={t.accent} />
            </Pressable>
          ),
        }}
      />
      <View style={styles.statusBar}>
        <ConnectionBadge status={connection.status} detail={connection.detail} />
        <Text style={{ color: t.textTertiary, fontSize: 12 }} numberOfLines={1}>
          {connection.desktop ? connection.desktop.name : 'No desktop'}
        </Text>
      </View>
      <FlashList
        data={rows}
        keyExtractor={(c) => c.id}
        contentContainerStyle={{ paddingBottom: spacing(8) }}
        contentInsetAdjustmentBehavior="automatic"
        ListEmptyComponent={
          connection.status === 'connected' ? (
            <EmptyState
              icon="bubble.left"
              title="No sessions yet"
              body="Start a session and it shows up here."
              actionLabel="New session"
              onAction={() => router.push('/new-session')}
            />
          ) : (
            <EmptyState
              icon="desktopcomputer"
              title={connection.status === 'disconnected' ? 'Not connected' : 'Connecting…'}
              body="Link a desktop to see its sessions."
              actionLabel="Link a desktop"
              onAction={() => router.push('/pair')}
            />
          )
        }
        renderItem={({ item: chat }) => {
          const ind = indicator(chat, sessions);
          const space = spaceName(chat);
          return (
            <Pressable
              onPress={() => {
                void client.markChatSeen(chat.id);
                router.push({ pathname: '/session/[id]', params: { id: chat.id, title: chat.title ?? 'Session' } });
              }}
              onLongPress={() => chatActions(client, chat)}
              style={({ pressed }) => [styles.chatRow, pressed && { backgroundColor: t.surface }]}
            >
              <View style={[styles.chatDot, { backgroundColor: statusColor(t, ind) }]} />
              <View style={styles.chatBody}>
                <Text style={[styles.chatTitle, { color: t.text }]} numberOfLines={1}>
                  {chat.title || 'Untitled session'}
                </Text>
                <Text style={{ color: t.textSecondary, fontSize: 13 }} numberOfLines={1}>
                  {chat.lastMessagePreview || space || chat.cwd || '—'}
                </Text>
              </View>
              <View style={styles.chatMeta}>
                <Text style={{ color: t.textTertiary, fontSize: 12 }}>{relTime(chat.lastMessageAt ?? chat.createdAt)}</Text>
                {ind === 'awaitingInput' ? (
                  <View style={[styles.badge, { backgroundColor: t.warning }]}>
                    <Text style={{ color: '#fff', fontSize: 10, fontWeight: '700' }}>INPUT</Text>
                  </View>
                ) : null}
              </View>
            </Pressable>
          );
        }}
        ItemSeparatorComponent={() => <View style={[styles.sep, { backgroundColor: t.separator }]} />}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  statusBar: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    paddingHorizontal: spacing(4), paddingVertical: spacing(2),
  },
  chatRow: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(4), paddingVertical: spacing(3),
  },
  chatDot: { width: 9, height: 9, borderRadius: 5 },
  chatBody: { flex: 1, minWidth: 0 },
  chatTitle: { fontSize: 16, fontWeight: '500' },
  chatMeta: { alignItems: 'flex-end', gap: 3 },
  badge: { borderRadius: radii.pill, paddingHorizontal: 6, paddingVertical: 2 },
  sep: { height: StyleSheet.hairlineWidth, marginLeft: spacing(4) + 9 + spacing(3) },
});
