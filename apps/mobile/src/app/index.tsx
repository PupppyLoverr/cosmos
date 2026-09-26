// Home (Codex / Cursor): big "Cosmos" title, host chips, sessions grouped by
// project as plain rows, and a floating bottom bar — search + "New".
// Navigation lives in the sidebar (menu button or swipe from the left edge).

import React, { useEffect, useMemo, useState } from 'react';
import { Alert, FlatList, Platform, Pressable, RefreshControl, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { Link, router } from 'expo-router';
import Animated, { useAnimatedKeyboard, useAnimatedStyle } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { LinearGradient } from 'expo-linear-gradient';
import { formatTimeAgo } from '../lib/activity';
import { desktopDisplayName, useCosmos, useSnapshot } from '../lib/client';
import { describeLink } from '../lib/connection-state';
import { Chat, Device, Session, Space } from '../lib/protocol';
import { showMenu } from '../lib/menu';
import { alpha, spacing, statusColor, type, useTheme } from '../lib/theme';
import { Button, EmptyState, Icon, Notice, StatusDot } from '../components/bits';
import { CircleButton, FilterChip, PillButton, ScreenTitle, TopBar } from '../components/chrome';
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
  | { kind: 'header'; key: string; title: string; icon?: string; count: number; collapsible?: boolean; collapsed?: boolean; tone?: 'attention' }
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
              {stateLabel ? <Text style={{ color: stateTone, fontWeight: '600' }}>{`  ${stateLabel}`}</Text> : null}
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
  const keyboard = useAnimatedKeyboard();
  const [query, setQuery] = useState('');
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
    if (attention.length) { out.push({ kind: 'header', key: 'h:attn', title: 'Needs you', count: attention.length, tone: 'attention' }); push(attention); }
    if (live.length) { out.push({ kind: 'header', key: 'h:live', title: 'Working', count: live.length }); push(live); }

    // Everything else grouped by project, most recently active project first.
    const groups = new Map<string, typeof pool>();
    for (const r of rest) {
      const p = projectOf(r.chat, spaces);
      groups.set(p, [...(groups.get(p) ?? []), r]);
    }
    for (const [project, rows] of groups) {
      const key = `p:${project}`;
      const isCollapsed = collapsed.has(key) && !q;
      out.push({ kind: 'header', key, title: project, icon: 'folder', count: rows.length, collapsible: true, collapsed: isCollapsed });
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
    { label: showArchived ? 'Hide archived' : 'Show archived', onPress: () => setShowArchived((v) => !v) },
    { label: 'Desktops', onPress: () => router.push('/desktops') },
    { label: 'Pair a desktop', onPress: () => router.push('/pair') },
    { label: 'Refresh', onPress: () => void refresh() },
  ]);

  const needsYou = sessions.some((s) => s.status === 'awaitingInput' || s.status === 'errored');
  const barStyle = useAnimatedStyle(() => ({
    transform: [{ translateY: -Math.max(0, keyboard.height.get() - insets.bottom) }],
  }));
  const desktopName = connection.desktop ? desktopDisplayName(connection.desktop) : 'your desktop';
  const link = describeLink(connection, desktopName);

  const header = (
    <View>
      <ScreenTitle>Cosmos</ScreenTitle>
      <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.chips}>
        <FilterChip label="All" selected={host === 'all'} onPress={() => setHost('all')} />
        {hosts.map((h) => (
          <FilterChip
            key={h.id}
            label={h.name}
            dot={h.dot}
            icon="laptopcomputer"
            selected={host === h.id}
            onPress={() => setHost(host === h.id ? 'all' : h.id)}
            onLongPress={() => router.push('/desktops')}
          />
        ))}
        <FilterChip label="Pair" icon="plus" onPress={() => router.push('/pair')} />
      </ScrollView>
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
        right={<CircleButton icon="ellipsis" label="More" onPress={more} />}
      />
      <FlatList
        data={items}
        keyExtractor={(i) => i.key}
        initialNumToRender={14}
        maxToRenderPerBatch={10}
        windowSize={9}
        ListHeaderComponent={header}
        contentContainerStyle={{ paddingBottom: insets.bottom + 120 }}
        keyboardDismissMode="on-drag"
        keyboardShouldPersistTaps="handled"
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} tintColor={t.textTertiary} />}
        ListEmptyComponent={
          !connection.desktop ? (
            <EmptyState title="Pair a desktop to start" body="Open Cosmos on your computer and scan its code." actionLabel="Pair this phone" onAction={() => router.push('/pair')} />
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
            <EmptyState title={`Nothing matches “${query}”`} />
          ) : (
            <EmptyState title="What should we work on?" body="Start a session and it shows up here." />
          )
        }
        renderItem={({ item }) => {
          if (item.kind === 'header') {
            const content = (
              <>
                {item.icon ? <Icon name={item.icon} size={18} color={t.text} /> : null}
                <Text style={[styles.headerText, { color: item.tone === 'attention' ? t.warning : t.text }]} numberOfLines={1}>{item.title}</Text>
                {item.collapsible ? <Icon name={item.collapsed ? 'chevron.right' : 'chevron.down'} size={12} color={t.textTertiary} weight="semibold" /> : null}
                <View style={styles.flex} />
                <Text style={[type.footnote, { color: t.textTertiary }]}>{item.count}</Text>
              </>
            );
            return item.collapsible ? (
              <Pressable
                onPress={() => { void Haptics.selectionAsync(); toggle(item.key); }}
                accessibilityRole="button"
                accessibilityState={{ expanded: !item.collapsed }}
                style={styles.header}
              >
                {content}
              </Pressable>
            ) : <View style={styles.header} accessibilityRole="header">{content}</View>;
          }
          return <SessionItem chat={item.chat} state={item.state} desktop={item.desktop} startedAt={item.startedAt} />;
        }}
      />

      <LinearGradient
        pointerEvents="none"
        colors={[alpha(t.background, 0), t.background]}
        style={[styles.fade, { height: insets.bottom + 120 }]}
      />
      <Animated.View style={[styles.bar, { paddingBottom: insets.bottom + spacing(2) }, barStyle]} pointerEvents="box-none">
        <View style={[styles.search, { backgroundColor: t.floating, boxShadow: t.floatShadow }]}>
          <Icon name="magnifyingglass" size={18} color={t.textSecondary} />
          <TextInput
            value={query}
            onChangeText={setQuery}
            placeholder="Search sessions"
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
        <PillButton title="New" icon="square.and.pencil" onPress={() => router.push('/new-session')} disabled={!connected} />
      </Animated.View>
      <DrawerEdge />
    </Screen>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  chips: { flexDirection: 'row', gap: spacing(2), paddingHorizontal: spacing(4), paddingBottom: spacing(3) },
  notice: { paddingHorizontal: spacing(4), paddingBottom: spacing(2) },
  noticeAction: { alignSelf: 'flex-start', marginTop: spacing(2) },
  header: { flexDirection: 'row', alignItems: 'center', gap: spacing(2.5), minHeight: 44, paddingHorizontal: spacing(4), marginTop: spacing(3) },
  headerText: { fontSize: 17, fontWeight: '600', flexShrink: 1 },
  row: { flexDirection: 'row', alignItems: 'flex-start', gap: spacing(3), paddingHorizontal: spacing(4), paddingVertical: spacing(3), minHeight: 64 },
  dotCol: { width: 18, alignItems: 'center', paddingTop: 8 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  rowCopy: { flex: 1, minWidth: 0, gap: 3 },
  rowTop: { flexDirection: 'row', alignItems: 'baseline', gap: spacing(2) },
  rowTitle: { flex: 1, fontSize: 17, lineHeight: 22 },
  fade: { position: 'absolute', left: 0, right: 0, bottom: 0 },
  bar: { position: 'absolute', left: 0, right: 0, bottom: 0, flexDirection: 'row', alignItems: 'center', gap: spacing(3), paddingHorizontal: spacing(4) },
  search: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: spacing(2), minHeight: 52, paddingHorizontal: spacing(4), borderRadius: 999 },
  searchInput: { flex: 1, paddingVertical: spacing(2) },
});
