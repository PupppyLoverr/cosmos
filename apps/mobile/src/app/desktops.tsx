import React, { useEffect } from 'react';
import { Alert, Platform, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { Link, router } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { formatTimeAgo } from '../lib/activity';
import { desktopDisplayName, pairingExpired, useCosmos, useSnapshot, LinkedDesktop } from '../lib/client';
import { describeLink } from '../lib/connection-state';
import { Device } from '../lib/protocol';
import { showMenu } from '../lib/menu';
import { spacing, type, useTheme } from '../lib/theme';
import { EmptyState, Icon } from '../components/bits';
import { CircleButton, PillButton, ScreenTitle, TopBar } from '../components/chrome';
import { Screen } from '../components/surface';

function platformIcon(platform?: string): string {
  const p = (platform ?? '').toLowerCase();
  if (p.includes('mac') || p.includes('darwin')) return 'laptopcomputer';
  if (p.includes('win')) return 'pc';
  if (p.includes('linux')) return 'server.rack';
  return 'desktopcomputer';
}

function platformName(platform?: string): string | undefined {
  const p = (platform ?? '').toLowerCase();
  if (p.includes('mac') || p.includes('darwin')) return 'Mac';
  if (p.includes('win')) return 'Windows';
  if (p.includes('linux')) return 'Linux';
  return undefined;
}

type Presence = 'online' | 'connecting' | 'asleep' | 'repair' | 'offline' | 'unknown';

function DesktopRow({ desktop, device, presence, active, onPress }: {
  desktop: LinkedDesktop; device?: Device; presence: Presence; active: boolean; onPress: () => void;
}) {
  const t = useTheme();
  const kind = [platformName(device?.platform), desktop.kind === 'local' ? 'This network' : 'Remote'].filter(Boolean).join(' · ');
  const seen = presence === 'online' ? (active ? 'Connected' : 'Online')
    : presence === 'connecting' ? 'Connecting…'
    : presence === 'repair' ? 'Pair again'
    : presence === 'asleep' ? 'Asleep · reconnects on wake'
    : device?.lastSeenAt ? `Seen ${formatTimeAgo(device.lastSeenAt)}`
    : presence === 'offline' ? 'Offline' : undefined;
  const dot = presence === 'online' ? t.success
    : presence === 'connecting' ? t.warning
    : presence === 'repair' ? t.danger
    : presence === 'asleep' ? t.textTertiary : t.separator;
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={[desktopDisplayName(desktop), kind, seen].filter(Boolean).join(', ')}
      style={({ pressed }) => [styles.row, { backgroundColor: pressed ? t.surface : t.background }]}
    >
      <View style={[styles.icon, { backgroundColor: t.surface }]}>
        <Icon name={device ? platformIcon(device.platform) : desktop.kind === 'local' ? 'wifi' : 'globe'} size={20} color={t.text} />
        <View style={[styles.dot, { backgroundColor: dot, borderColor: t.background }]} />
      </View>
      <View style={styles.copy}>
        <Text style={[styles.name, { color: t.text }]} numberOfLines={1}>{desktopDisplayName(desktop)}</Text>
        <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>{[kind, seen].filter(Boolean).join(' · ')}</Text>
      </View>
      {active ? <Icon name="checkmark" size={16} color={t.text} weight="semibold" /> : null}
    </Pressable>
  );
}

export default function DesktopsScreen() {
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const client = useCosmos();
  const desktops = useSnapshot(client.desktops);
  const connection = useSnapshot(client.connection);
  const devices = useSnapshot(client.devices);
  const auth = useSnapshot(client.auth);

  // Remote rows show relay presence (the desktop process checked in), probed
  // from the edge so the list is truthful even while this phone is offline.
  const remoteOnline = useSnapshot(client.remotePresence);
  const remoteAddrs = desktops.filter((d) => d.kind === 'remote').map((d) => d.address).join(',');
  useEffect(() => {
    if (!auth) return;
    void client.refreshRemotePresence();
    const timer = setInterval(() => void client.refreshRemotePresence(), 20_000);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auth, remoteAddrs]);

  const activeId = connection.desktop?.id;
  const ordered = [...desktops].sort((a, b) => Number(b.id === activeId) - Number(a.id === activeId));

  const presenceOf = (d: LinkedDesktop): Presence => {
    if (pairingExpired(d)) return 'repair';
    if (d.id === activeId) {
      const link = describeLink(connection);
      if (link.needsPair) return 'repair';
      if (link.tone === 'online') return 'online';
      if (link.tone === 'asleep') return 'asleep';
      if (link.tone === 'pending') return 'connecting';
      return 'offline';
    }
    if (d.kind === 'remote') return remoteOnline[d.address] === true ? 'online' : remoteOnline[d.address] === false ? 'asleep' : 'unknown';
    return 'unknown';
  };

  const unlink = (d: LinkedDesktop) => {
    Alert.alert('Unlink this desktop?', 'Removes the pairing from this phone. The desktop keeps its own settings.', [
      { text: 'Cancel', style: 'cancel' },
      { text: 'Unlink', style: 'destructive', onPress: () => void client.removeDesktop(d.id) },
    ]);
  };
  const rename = (d: LinkedDesktop) =>
    Alert.prompt('Rename desktop', undefined, (name) => { if (name?.trim()) void client.renameDesktop(d.id, name.trim()); }, 'plain-text', d.name);
  const actions = (d: LinkedDesktop) => showMenu(desktopDisplayName(d), [
    { label: 'Connect', onPress: () => void client.connect(d) },
    ...(Platform.OS === 'ios' ? [{ label: 'Rename…', onPress: () => rename(d) }] : []),
    { label: 'Unlink', destructive: true, onPress: () => unlink(d) },
  ]);

  return (
    <Screen>
      <TopBar left={<CircleButton icon="chevron.left" label="Back" onPress={() => router.back()} />} />
      <ScrollView contentContainerStyle={{ paddingBottom: insets.bottom + 120 }}>
        <ScreenTitle>Desktops</ScreenTitle>
        {desktops.length === 0 ? (
          <EmptyState title="No desktops yet" body="Open Cosmos on your computer, then pair this phone." />
        ) : (
          ordered.map((d) => {
            const device = devices.find((x) => x.id === d.id || x.id === d.address)
              ?? (d.id === activeId ? devices.find((x) => x.id === connection.localDeviceId) : undefined);
            const row = (
              <DesktopRow
                desktop={d}
                device={device}
                presence={presenceOf(d)}
                active={d.id === activeId}
                onPress={() => {
                  void Haptics.selectionAsync();
                  if (presenceOf(d) === 'repair') router.push('/pair');
                  else if (Platform.OS === 'ios') void client.connect(d);
                  else actions(d);
                }}
              />
            );
            if (Platform.OS !== 'ios') return <View key={d.id}>{row}</View>;
            return (
              <Link key={d.id} href="/desktops" asChild>
                <Link.Trigger>{row}</Link.Trigger>
                <Link.Menu title={desktopDisplayName(d)}>
                  <Link.MenuAction title="Connect" icon="link" onPress={() => void client.connect(d)} />
                  <Link.MenuAction title="Rename…" icon="pencil" onPress={() => rename(d)} />
                  <Link.MenuAction title="Unlink" icon="trash" destructive onPress={() => unlink(d)} />
                </Link.Menu>
              </Link>
            );
          })
        )}

        <Text style={[type.subhead, styles.section, { color: t.textTertiary }]}>Other ways to connect</Text>
        <Pressable
          onPress={() => router.push({ pathname: '/pair', params: { mode: 'local' } })}
          accessibilityRole="button"
          style={({ pressed }) => [styles.row, { backgroundColor: pressed ? t.surface : t.background }]}
        >
          <View style={[styles.icon, { backgroundColor: t.surface }]}><Icon name="wifi" size={20} color={t.text} /></View>
          <View style={styles.copy}>
            <Text style={[styles.name, { color: t.text }]}>On this network</Text>
            <Text style={[type.footnote, { color: t.textSecondary }]}>Link by address over one Wi-Fi</Text>
          </View>
          <Icon name="chevron.right" size={13} color={t.textTertiary} weight="semibold" />
        </Pressable>
        {!auth ? (
          <Pressable
            onPress={() => router.push('/sign-in')}
            accessibilityRole="button"
            style={({ pressed }) => [styles.row, { backgroundColor: pressed ? t.surface : t.background }]}
          >
            <View style={[styles.icon, { backgroundColor: t.surface }]}><Icon name="person.crop.circle" size={20} color={t.text} /></View>
            <View style={styles.copy}>
              <Text style={[styles.name, { color: t.text }]}>Sign in to Woozlit</Text>
              <Text style={[type.footnote, { color: t.textSecondary }]}>Reach your desktops from anywhere</Text>
            </View>
            <Icon name="chevron.right" size={13} color={t.textTertiary} weight="semibold" />
          </Pressable>
        ) : null}
      </ScrollView>

      <View style={[styles.bar, { paddingBottom: insets.bottom + spacing(2) }]} pointerEvents="box-none">
        <PillButton title="Pair phone" icon="qrcode.viewfinder" onPress={() => router.push('/pair')} style={styles.pill} />
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  row: { flexDirection: 'row', alignItems: 'center', gap: spacing(3.5), paddingHorizontal: spacing(4), paddingVertical: spacing(3), minHeight: 68 },
  icon: { width: 44, height: 44, borderRadius: 12, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  dot: { position: 'absolute', right: -2, bottom: -2, width: 12, height: 12, borderRadius: 6, borderWidth: 2 },
  copy: { flex: 1, minWidth: 0, gap: 2 },
  name: { fontSize: 17, fontWeight: '500', lineHeight: 22 },
  section: { paddingHorizontal: spacing(4), marginTop: spacing(8), marginBottom: spacing(1) },
  bar: { position: 'absolute', left: 0, right: 0, bottom: 0, paddingHorizontal: spacing(4) },
  pill: { alignSelf: 'stretch' },
});
