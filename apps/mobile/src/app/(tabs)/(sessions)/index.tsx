import React, { useEffect, useMemo, useState } from 'react';
import { Alert, Platform, Pressable, RefreshControl, ScrollView, StyleSheet, Text, View } from 'react-native';
import { FlashList } from '@shopify/flash-list';
import { router, Stack } from 'expo-router';
import { SafeAreaView } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { formatTimeAgo } from '../../../lib/activity';
import { desktopDisplayName, useCosmos, useSnapshot, type ConnectionInfo, type LinkedDesktop } from '../../../lib/client';
import { Chat, Device, Session } from '../../../lib/protocol';
import { edge, radii, spacing, type, useTheme } from '../../../lib/theme';
import { DesktopChip, EmptyState, Icon, Notice, SkeletonRows } from '../../../components/bits';
import { Card, Screen } from '../../../components/surface';
import { SessionRow, SessionRowIndicator } from '../../../components/session-row';

function indicator(chat: Chat, sessions: Session[]): SessionRowIndicator {
  const live = sessions.find((s) => s.chatId === chat.id);
  if (live?.status === 'working') return 'working';
  if (live?.status === 'awaitingInput') return 'awaitingInput';
  if (live?.status === 'errored') return 'errored';
  if (chat.lastMessageAt && (!chat.lastSeenAt || chat.lastMessageAt > chat.lastSeenAt)) return 'completed';
  return 'idle';
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
  | { kind: 'section'; key: string; title: string; count: number; attention?: boolean }
  | { kind: 'archive'; key: string; count: number; open: boolean }
  | { kind: 'chat'; key: string; chat: Chat; ind: SessionRowIndicator; desktop: string; startedAt?: string | null };

function listItemKeyExtractor(item: ListItem): string {
  return item.key;
}

function listItemType(item: ListItem): ListItem['kind'] {
  return item.kind;
}

/** Host chips (Codex): every linked desktop, the connected one with a green dot. Tap → Desktops. */
function DesktopStrip({ desktops, connection }: { desktops: LinkedDesktop[]; connection: ConnectionInfo }) {
  const openDesktops = () => router.push('/(tabs)/(desktops)' as never);
  if (desktops.length === 0) {
    return (
      <View style={styles.strip}>
        <DesktopChip name="No desktop linked" state="offline" onPress={() => router.push('/pair')} />
      </View>
    );
  }
  const connecting = connection.status === 'connecting' || connection.status === 'reconnecting';
  // Connected desktop first, the rest in the order they were linked.
  const ordered = [...desktops].sort((a, b) => Number(b.id === connection.desktop?.id) - Number(a.id === connection.desktop?.id));
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.strip}>
      {ordered.map((d) => {
        const active = d.id === connection.desktop?.id;
        const state = active && connection.status === 'connected' ? 'online' : active && connecting ? 'connecting' : 'offline';
        return <DesktopChip key={d.id} name={desktopDisplayName(d)} state={state} onPress={openDesktops} />;
      })}
    </ScrollView>
  );
}

/** Cursor-style docked composer: a card over the list that opens the New Session sheet. */
function ComposerDock() {
  const t = useTheme();
  return (
    <SafeAreaView edges={['bottom']} style={styles.dockWrap} pointerEvents="box-none">
      <Pressable
        onPress={() => { void Haptics.selectionAsync(); router.push('/new-session'); }}
        accessibilityRole="button"
        accessibilityLabel="New session"
        style={({ pressed }) => [styles.dockPress, { opacity: pressed ? 0.85 : 1 }]}
      >
        <Card radius={radii.composer} style={[styles.dock, { boxShadow: t.scheme === 'dark' ? undefined : '0 6px 20px rgba(0,0,0,0.08)' }]}>
          <Icon name="plus" size={18} color={t.textSecondary} />
          <Text style={[type.body, { color: t.textTertiary, flex: 1 }]} numberOfLines={1}>What should we do on the desktop?</Text>
          <View style={[styles.dockSend, { backgroundColor: t.text }]}>
            <Icon name="arrow.up" size={15} color={t.background} weight="bold" />
          </View>
        </Card>
      </Pressable>
    </SafeAreaView>
  );
}

export default function SessionsScreen() {
  const t = useTheme();
  const client = useCosmos();
  const chats = useSnapshot(client.chats);
  const sessions = useSnapshot(client.sessions);
  const devices = useSnapshot(client.devices);
  const desktops = useSnapshot(client.desktops);
  const connection = useSnapshot(client.connection);
  const [query, setQuery] = useState('');
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [tick, setTick] = useState(0);
  const anyWorking = sessions.some((session) => session.status === 'working');

  useEffect(() => {
    if (!anyWorking) return;
    const interval = setInterval(() => setTick((value) => value + 1), 30_000);
    return () => clearInterval(interval);
  }, [anyWorking]);

  const items = useMemo<ListItem[]>(() => {
    const q = query.trim().toLowerCase();
    const sessionByChat = new Map(sessions.map((session) => [session.chatId, session]));
    const enriched = chats.map((chat) => ({ chat, desktop: deviceName(chat, devices) }));
    const matches = (value: (typeof enriched)[number]) => (
      !q
      || (value.chat.title ?? '').toLowerCase().includes(q)
      || (value.chat.lastMessagePreview ?? '').toLowerCase().includes(q)
      || (value.chat.branch ?? '').toLowerCase().includes(q)
      || value.desktop.toLowerCase().includes(q)
    );
    const byRecent = (a: (typeof enriched)[number], b: (typeof enriched)[number]) =>
      (b.chat.lastMessageAt ?? b.chat.createdAt).localeCompare(a.chat.lastMessageAt ?? a.chat.createdAt);
    const live = enriched
      .filter((value) => !value.chat.archived && matches(value))
      .sort(byRecent)
      .map((value) => {
        const session = sessionByChat.get(value.chat.id);
        return { ...value, ind: indicator(value.chat, sessions), startedAt: session?.status === 'working' ? session.startedAt : undefined };
      });
    const archived = enriched.filter((value) => value.chat.archived && matches(value)).sort(byRecent);

    const attention = live.filter((value) => value.ind === 'awaitingInput' || value.ind === 'errored');
    const working = live.filter((value) => value.ind === 'working');
    const recent = live.filter((value) => value.ind === 'completed' || value.ind === 'idle');
    const headed = [attention.length, working.length, recent.length, archived.length].filter(Boolean).length > 1;
    const out: ListItem[] = [];
    const pushChats = (title: string, group: typeof live, attn = false) => {
      if (!group.length) return;
      if (headed) out.push({ kind: 'section', key: `s:${title}`, title, count: group.length, attention: attn });
      for (const value of group) {
        out.push({ kind: 'chat', key: value.chat.id, chat: value.chat, ind: value.ind, desktop: value.desktop, startedAt: value.startedAt });
      }
    };
    pushChats('Needs you', attention, true);
    pushChats('Live', working);
    pushChats('Recent', recent);
    if (archived.length) {
      const open = archiveOpen || q.length > 0;
      out.push({ kind: 'archive', key: 'archive', count: archived.length, open });
      if (open) {
        for (const value of archived) {
          out.push({ kind: 'chat', key: `arch:${value.chat.id}`, chat: value.chat, ind: 'idle', desktop: value.desktop });
        }
      }
    }
    return out;
  }, [archiveOpen, chats, devices, query, sessions]);

  const connected = connection.status === 'connected';
  const connecting = connection.status === 'connecting' || connection.status === 'reconnecting';
  const refresh = async () => {
    setRefreshing(true);
    try { await client.refreshRemotePresence(); } catch { /* ignore */ }
    setTimeout(() => setRefreshing(false), 600);
  };
  const desktopName = connection.desktop ? desktopDisplayName(connection.desktop) : 'the desktop';

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
        }}
      />
      <FlashList
        data={items}
        extraData={tick}
        keyExtractor={listItemKeyExtractor}
        getItemType={listItemType}
        contentContainerStyle={{ ...styles.list, paddingBottom: connected ? spacing(28) : spacing(12) }}
        contentInsetAdjustmentBehavior="automatic"
        keyboardDismissMode="on-drag"
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} tintColor={t.textTertiary} />}
        ListHeaderComponent={
          <View style={styles.header}>
            <DesktopStrip desktops={desktops} connection={connection} />
            {!connected && connection.desktop && connection.detail ? (
              <Notice kind="warning" title={connecting ? 'Connecting…' : `Can't reach ${desktopName}`} body={connection.detail} />
            ) : null}
          </View>
        }
        ListEmptyComponent={
          connection.status === 'connecting' ? (
            <SkeletonRows />
          ) : connected ? (
            query ? (
              <EmptyState title={`Nothing matches “${query}”`} />
            ) : (
              <EmptyState title="No sessions yet" actionLabel="New session" onAction={() => router.push('/new-session')} />
            )
          ) : !connection.desktop ? (
            <EmptyState title="Pair a desktop to start" actionLabel="Pair this phone" onAction={() => router.push('/pair')} />
          ) : connecting ? (
            <EmptyState title={`Connecting to ${desktopName}…`} />
          ) : (
            <EmptyState title={`Can't reach ${desktopName}`} actionLabel="Retry" onAction={() => { if (connection.desktop) void client.connect(connection.desktop); }} />
          )
        }
        renderItem={({ item }) => {
          if (item.kind === 'section') {
            return (
              <View style={styles.section}>
                <Text accessibilityRole="header" style={[type.footnote, { color: item.attention ? t.warning : t.textSecondary, fontWeight: '600' }]}>
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
                <Text style={[type.footnote, { color: t.textSecondary, fontWeight: '600' }]}>Archived</Text>
                <Text style={[type.footnote, { color: t.textTertiary }]}>{item.count}</Text>
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
      {connected ? <ComposerDock /> : null}
    </Screen>
  );
}

const styles = StyleSheet.create({
  list: { paddingHorizontal: edge },
  header: { gap: spacing(3), paddingTop: spacing(1), paddingBottom: spacing(3) },
  strip: { flexDirection: 'row', gap: spacing(2) },
  section: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginTop: spacing(3),
    marginBottom: spacing(2),
    paddingHorizontal: spacing(1),
  },
  archive: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), minHeight: 44, marginTop: spacing(2), paddingHorizontal: spacing(1) },
  dockWrap: { position: 'absolute', left: 0, right: 0, bottom: 0, paddingHorizontal: spacing(3) },
  dockPress: { paddingBottom: spacing(2) },
  dock: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), minHeight: 56, paddingLeft: spacing(4), paddingRight: spacing(2.5) },
  dockSend: { width: 36, height: 36, borderRadius: 18, alignItems: 'center', justifyContent: 'center' },
});
