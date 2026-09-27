// Home (Codex / Cursor): big "Cosmos" title, host chips, sessions grouped by
// project as plain rows, and a floating bottom bar — search + "New".
// Navigation lives in the sidebar (menu button or swipe from the left edge).

import React, { useEffect, useMemo, useState } from 'react';
import { Alert, FlatList, Platform, Pressable, RefreshControl, StyleSheet, Text, TextInput, View } from 'react-native';
import { Link, router } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { formatTimeAgo } from '../lib/activity';
import { desktopDisplayName, useCosmos, useSnapshot } from '../lib/client';
import { describeLink } from '../lib/connection-state';
import { Chat, Device, Session, Space } from '../lib/protocol';
import { showMenu } from '../lib/menu';
import { edge, spacing, statusColor, type, useTheme } from '../lib/theme';
import { Button, EmptyState, Notice, StatusDot } from '../components/bits';
import { CircleButton, DockPill, ScreenTitle, SectionLabel, TopBar } from '../components/chrome';
import { DrawerEdge, useDrawer } from '../components/drawer';
import { harnessLabel } from '../components/harness-mark';
import { Screen } from '../components/surface';

type State = 'working' | 'awaitingInput' | 'errored' | 'unread' | 'idle';

function stateOf(chat: Chat, live: Session | undefined): State {
  if (live?.status === 'working') return 'working';
  if (live?.status === 'awaitingInput') return 'awaitingInput';
  if (live?.status === 'errored') return 'errored';
  if (chat.lastMessageAt && (!chat.lastSeenAt || chat.lastMessageAt > chat.lastSeenAt)) return 'unread';
  return 'idle';
}

function projectOf(chat: Chat, spaces: Space[]): string {
  const space = chat.spaceId ? spaces.find((s) => s.id === chat.spaceId) : undefined;
  if (space?.name?.trim()) return space.name.trim();
  const path = (space?.path ?? chat.cwd ?? '').replace(/\\/g, '/');
  if (!path || path === '~' || path === '~/' || /^\/(Users|home)\/[^/]+\/?$/.test(path) || /^[A-Za-z]:\/Users\/[^/]+\/?$/.test(path)) return path ? 'Home' : 'No project';
  return path.split('/').filter(Boolean).pop() || 'No project';
}

function elapsed(startedAt?: string | null): string | undefined {
  if (!startedAt) return undefined;
  const minutes = Math.floor(Math.max(0, Date.now() - new Date(startedAt).getTime()) / 60_000);
  return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

type Item =
  | { kind: 'header'; key: string; title: string; collapsible?: boolean; collapsed?: boolean; tone?: 'attention' }
  | { kind: 'chat'; key: string; chat: Chat; state: State; desktop?: string; startedAt?: string | null };

/** One session: status dot, title, "harness · desktop · state", time on the right. */
const SessionItem = React.memo(function SessionItem({ chat, state, desktop, startedAt }: { chat: Chat; state: State; desktop?: string; startedAt?: string | null }) {
  const t = useTheme();
  const client = useCosmos();
  const title = chat.title || chat.lastMessagePreview || 'Untitled session';
  const stateLabel = state === 'working' ? `Working${elapsed(startedAt) ? ` ${elapsed(startedAt)}` : ''}`
    : state === 'awaitingInput' ? 'Needs you'
    : state === 'errored' ? 'Failed'
    : undefined;
  const stateTone = state === 'working' ? t.link : state === 'awaitingInput' ? t.warning : state === 'errored' ? t.danger : undefined;
  const meta = [harnessLabel(chat.config?.harness), desktop].filter(Boolean).join(' · ');
  const dot = state === 'idle' ? t.separator : state === 'unread' ? t.link : statusColor(t, state);

  const rename = () => Alert.prompt?.('Rename session', undefined, (v) => { if (v?.trim()) void client.renameChat(chat.id, v.trim()); }, 'plain-text', chat.title ?? '');
  const archive = () => void client.setChatArchived(chat.id, !chat.archived);
  const remove = () => Alert.alert('Delete session?', 'This removes the chat from the desktop too.', [
    { text: 'Cancel', style: 'cancel' },
    { text: 'Delete', style: 'destructive', onPress: () => void client.deleteChat(chat.id) },
  ]);

  return (
    <Link asChild href={{ pathname: '/session/[id]', params: { id: chat.id, title } }}>
      <Link.Trigger>
        <Pressable
          onPress={() => { void client.markChatSeen(chat.id); }}
          onLongPress={Platform.OS !== 'ios' ? () => {
            void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium);
            showMenu(title, [
              { label: 'Rename', onPress: rename },
              { label: chat.archived ? 'Unarchive' : 'Archive', onPress: archive },
              { label: 'Delete', destructive: true, onPress: remove },
            ]);
          } : undefined}
          accessibilityRole="button"
          accessibilityLabel={[title, stateLabel, meta, formatTimeAgo(chat.lastMessageAt ?? chat.createdAt)].filter(Boolean).join(', ')}
          style={({ pressed }) => ({ backgroundColor: pressed ? t.surface : t.background, opacity: chat.archived ? 0.55 : 1 })}
        >
          <View style={styles.row}>
          <View style={styles.dotCol}>
            {state === 'working' ? <StatusDot status="working" size={8} pulse /> : <View style={[styles.dot, { backgroundColor: dot }]} />}
          </View>
          <View style={styles.rowCopy}>
            <View style={styles.rowTop}>
              <Text style={[styles.rowTitle, { color: t.text, fontWeight: state === 'unread' ? '600' : '500' }]} numberOfLines={1}>{title}</Text>
              <Text style={[type.footnote, { color: t.textTertiary }]}>{formatTimeAgo(chat.lastMessageAt ?? chat.createdAt)}</Text>
            </View>
            <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>
              {meta}
              {' · '}
              {stateLabel ? (
                <Text style={{ color: stateTone, fontWeight: '500' }}>{stateLabel}</Text>
              ) : (
                <Text style={{ color: t.textSecondary }}>Idle</Text>
              )}
            </Text>
          </View>
          </View>
        </Pressable>
      </Link.Trigger>
      <Link.Menu title={title}>
        <Link.MenuAction title="Rename" icon="pencil" onPress={rename} />
        <Link.MenuAction title={chat.archived ? 'Unarchive' : 'Archive'} icon={chat.archived ? 'tray.and.arrow.up' : 'archivebox'} onPress={archive} />
        <Link.MenuAction title="Delete" icon="trash" destructive onPress={remove} />
      </Link.Menu>
      <Link.Preview />
    </Link>
  );
});

export default function HomeScreen() {
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const drawer = useDrawer();
  const client = useCosmos();
  const chats = useSnapshot(client.chats);
  const sessions = useSnapshot(client.sessions);
  const spaces = useSnapshot(client.spaces);
  const devices = useSnapshot(client.devices);
  const connection = useSnapshot(client.connection);
  const [query, setQuery] = useState('');
  const [searchOpen, setSearchOpen] = useState(false);
  const [host, setHost] = useState<string>('all');
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [showArchived, setShowArchived] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [, setTick] = useState(0);

  const connected = connection.status === 'connected';
  const connecting = connection.status === 'connecting' || connection.status === 'reconnecting';
  const anyWorking = sessions.some((s) => s.status === 'working');
  useEffect(() => {
    if (!anyWorking) return;
    const id = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(id);
  }, [anyWorking]);

  // Host chips: the engine's device registry; the connected one gets the green dot.
  const hosts = useMemo(() => {
    const list: { id: string; name: string; dot: 'online' | 'connecting' | 'offline' }[] = devices.map((d: Device) => ({
      id: d.id,
      name: d.name?.trim() || 'Desktop',
      dot: d.id === connection.localDeviceId ? (connected ? 'online' : connecting ? 'connecting' : 'offline') : 'offline',
    }));
    if (!list.length && connection.desktop) {
      list.push({ id: connection.localDeviceId ?? connection.desktop.id, name: desktopDisplayName(connection.desktop), dot: connected ? 'online' : connecting ? 'connecting' : 'offline' });
    }
    return list.sort((a, b) => Number(b.dot === 'online') - Number(a.dot === 'online'));
  }, [devices, connection, connected, connecting]);

  const items = useMemo<Item[]>(() => {
    const q = query.trim().toLowerCase();
    const deviceNames = new Map(devices.map((d) => [d.id, d.name?.trim() || 'Desktop']));
    const liveByChat = new Map<string, Session>();
    for (const s of sessions) if (!liveByChat.has(s.chatId)) liveByChat.set(s.chatId, s);
    // With a single desktop the host name on every row is noise.
    const multiHost = new Set(chats.map((c) => c.deviceId)).size > 1;
    const nameOf = (c: Chat) => (multiHost ? deviceNames.get(c.deviceId) ?? 'Desktop' : undefined);
    const pool = chats
      .filter((c) => (showArchived || q ? true : !c.archived))
      .filter((c) => host === 'all' || c.deviceId === host)
      .filter((c) => !q
        || (c.title ?? '').toLowerCase().includes(q)
        || (c.lastMessagePreview ?? '').toLowerCase().includes(q)
        || projectOf(c, spaces).toLowerCase().includes(q))
      .sort((a, b) => (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt))
      .map((chat) => {
        const live = liveByChat.get(chat.id);
        return {
          chat,
          state: stateOf(chat, live),
          desktop: nameOf(chat),
          startedAt: live?.status === 'working' ? live.startedAt : undefined,
        };
      });

    const out: Item[] = [];
    const push = (rows: typeof pool) => rows.forEach((r) => out.push({ kind: 'chat', key: `${out.length}:${r.chat.id}`, ...r }));
    const attention = pool.filter((r) => r.state === 'awaitingInput' || r.state === 'errored');
    const live = pool.filter((r) => r.state === 'working');
    const rest = pool.filter((r) => !attention.includes(r) && !live.includes(r));
    if (attention.length) { out.push({ kind: 'header', key: 'h:attn', title: 'Needs you', tone: 'attention' }); push(attention); }
    if (live.length) { out.push({ kind: 'header', key: 'h:live', title: 'Working' }); push(live); }

    // Everything else grouped by project, most recently active project first.
    const groups = new Map<string, typeof pool>();
    for (const r of rest) {
      const p = projectOf(r.chat, spaces);
      groups.set(p, [...(groups.get(p) ?? []), r]);
    }
    for (const [project, rows] of groups) {
      const key = `p:${project}`;
      const isCollapsed = collapsed.has(key) && !q;
      out.push({ kind: 'header', key, title: project, collapsible: true, collapsed: isCollapsed });
      if (!isCollapsed) push(rows);
    }
    return out;
  }, [chats, collapsed, devices, host, query, sessions, showArchived, spaces]);

  const toggle = (key: string) => setCollapsed((prev) => {
    const next = new Set(prev);
    if (next.has(key)) next.delete(key); else next.add(key);
    return next;
  });

  const refresh = async () => {
    setRefreshing(true);
    try { await client.refreshRemotePresence(); } catch { /* ignore */ }
    setTimeout(() => setRefreshing(false), 600);
  };

  const more = () => showMenu(undefined, [
    { label: 'All desktops', checked: host === 'all', onPress: () => setHost('all') },
    ...hosts.map((h) => ({ label: h.name, checked: host === h.id, onPress: () => setHost(h.id) })),
    { label: showArchived ? 'Hide archived' : 'Show archived', onPress: () => setShowArchived((v) => !v) },
    { label: 'Desktops', onPress: () => router.push('/desktops') },
    { label: 'Pair a desktop', onPress: () => router.push('/pair') },
    { label: 'Refresh', onPress: () => void refresh() },
  ]);

  const needsYou = sessions.some((s) => s.status === 'awaitingInput' || s.status === 'errored');
  const desktopName = connection.desktop ? desktopDisplayName(connection.desktop) : 'your desktop';
  const link = describeLink(connection, desktopName);

  const header = (
    <View>
      <ScreenTitle>{host === 'all' ? 'Sessions' : hosts.find((h) => h.id === host)?.name ?? 'Sessions'}</ScreenTitle>
      {searchOpen ? (
        <View style={[styles.searchRow, { backgroundColor: t.surface }]}>
          <TextInput
            autoFocus
            value={query}
            onChangeText={setQuery}
            placeholder="Search"
            placeholderTextColor={t.textTertiary}
            style={[type.body, styles.searchInput, { color: t.text }]}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="search"
            clearButtonMode="while-editing"
            keyboardAppearance={t.scheme}
            accessibilityLabel="Search sessions"
          />
        </View>
      ) : null}
      {!connected && connection.desktop && connection.detail && items.length > 0 ? (
        <View style={styles.notice}>
          <Notice
            kind={link.tone === 'attention' ? 'error' : link.tone === 'asleep' ? 'info' : 'warning'}
            title={link.title}
            body={link.body}
            action={link.needsPair ? (
              <Button title="Pair again" size="sm" variant="tinted" style={styles.noticeAction} onPress={() => router.push('/pair')} />
            ) : undefined}
          />
        </View>
      ) : null}
    </View>
  );

  return (
    <Screen>
      <TopBar
        left={<CircleButton icon="line.3.horizontal" label="Open menu" onPress={drawer.open} badge={needsYou} />}
        right={(
          <>
            <CircleButton icon="magnifyingglass" label="Search" onPress={() => setSearchOpen((v) => !v)} />
            <CircleButton icon="line.3.horizontal.decrease" label="Filter sessions" onPress={more} />
          </>
        )}
      />
      <FlatList
        data={items}
        keyExtractor={(i) => i.key}
        initialNumToRender={14}
        maxToRenderPerBatch={10}
        windowSize={9}
        ListHeaderComponent={header}
        contentContainerStyle={{ paddingBottom: insets.bottom + 90 }}
        keyboardDismissMode="on-drag"
        keyboardShouldPersistTaps="handled"
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} tintColor={t.textTertiary} />}
        ListEmptyComponent={
            !connection.desktop ? (
            <EmptyState title="No desktop paired" body="Open Cosmos on your computer and scan its code." actionLabel="Pair a desktop" onAction={() => router.push('/pair')} />
          ) : !connected ? (
            <EmptyState
              title={link.title}
              body={link.body}
              actionLabel={link.needsPair ? 'Pair again' : link.tone === 'pending' ? undefined : 'Try now'}
              onAction={() => {
                if (link.needsPair) router.push('/pair');
                else if (connection.desktop) void client.connect(connection.desktop);
              }}
            />
          ) : query ? (
            <EmptyState title={`No results for “${query}”`} />
          ) : (
            <EmptyState title="No sessions yet" body="Start one from the field below." />
          )
        }
        renderItem={({ item }) => {
          if (item.kind === 'header') {
            const content = <SectionLabel tone={item.collapsed ? 'muted' : item.tone}>{item.title}</SectionLabel>;
            return item.collapsible ? (
              <Pressable
                onPress={() => { void Haptics.selectionAsync(); toggle(item.key); }}
                accessibilityRole="button"
                accessibilityState={{ expanded: !item.collapsed }}
                style={styles.header}
              >
                {content}
              </Pressable>
            ) : content;
          }
          return <SessionItem chat={item.chat} state={item.state} desktop={item.desktop} startedAt={item.startedAt} />;
        }}
      />

      <View style={[styles.bar, { backgroundColor: t.background, borderTopColor: t.separator, paddingBottom: insets.bottom + spacing(2) }]} pointerEvents="box-none">
        <DockPill placeholder="Plan, ask, build…" onPress={() => router.push('/new-session')} disabled={!connected} />
      </View>
      <DrawerEdge />
    </Screen>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  notice: { paddingHorizontal: spacing(4), paddingBottom: spacing(2) },
  noticeAction: { alignSelf: 'flex-start', marginTop: spacing(2) },
  header: { paddingHorizontal: 0 },
  row: { flexDirection: 'row', alignItems: 'flex-start', gap: spacing(3), paddingHorizontal: spacing(4), paddingVertical: spacing(2.5), minHeight: 56 },
  dotCol: { width: 16, alignItems: 'center', paddingTop: 7 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  rowCopy: { flex: 1, minWidth: 0, gap: 3 },
  rowTop: { flexDirection: 'row', alignItems: 'baseline', gap: spacing(2) },
  rowTitle: { flex: 1, fontSize: 16, lineHeight: 21 },
  searchRow: { marginHorizontal: edge, minHeight: 40, borderRadius: 12, justifyContent: 'center', marginBottom: spacing(2) },
  searchInput: { flex: 1, paddingHorizontal: spacing(3), paddingVertical: spacing(1) },
  bar: { position: 'absolute', left: 0, right: 0, bottom: 0, paddingHorizontal: edge, paddingTop: spacing(2), borderTopWidth: StyleSheet.hairlineWidth },
});
