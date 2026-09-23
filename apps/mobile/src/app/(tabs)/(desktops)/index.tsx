import React, { useEffect } from 'react';
import { Alert, Platform, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { Link, router, Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { formatTimeAgo } from '../../../lib/activity';
import { desktopDisplayName, useCosmos, useSnapshot, LinkedDesktop } from '../../../lib/client';
import { Device } from '../../../lib/protocol';
import { edge, radii, spacing, type, useTheme } from '../../../lib/theme';
import { Button, EmptyState, Group, IconTile, Row, SectionLabel, StatusDot } from '../../../components/bits';
import { Card, Screen } from '../../../components/surface';

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

type Presence = 'online' | 'connecting' | 'offline' | 'unknown';

/** One desktop: name, kind, presence, last seen. Tap connects; long-press/menu for the rest. */
function DesktopCard({ desktop, device, presence, active, detail, onPress }: {
  desktop: LinkedDesktop; device?: Device; presence: Presence; active: boolean; detail?: string; onPress: () => void;
}) {
  const t = useTheme();
  const kind = [platformName(device?.platform), desktop.kind === 'local' ? 'On this network' : 'Remote'].filter(Boolean).join(' · ');
  const seen = presence === 'online' ? (active ? 'Connected' : 'Online')
    : presence === 'connecting' ? 'Connecting…'
    : device?.lastSeenAt ? `Last seen ${formatTimeAgo(device.lastSeenAt)}`
    : presence === 'offline' ? 'Offline' : undefined;
  const dot = presence === 'online' ? 'connected' : presence === 'connecting' ? 'awaitingInput' : undefined;
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={[desktopDisplayName(desktop), kind, seen].filter(Boolean).join(', ')}
      style={({ pressed }) => ({ opacity: pressed ? 0.7 : 1 })}
    >
      <Card radius={radii.lg} style={styles.card}>
        <IconTile name={device ? platformIcon(device.platform) : desktop.kind === 'local' ? 'wifi' : 'globe'} size={36} />
        <View style={styles.cardCopy}>
          <Text style={[type.sessionTitle, { color: t.text }]} numberOfLines={1}>{desktopDisplayName(desktop)}</Text>
          <Text style={[type.meta, { color: t.textSecondary }]} numberOfLines={1}>{kind}</Text>
          {active && detail && presence !== 'online' ? (
            <Text style={[type.meta, { color: t.warning }]} numberOfLines={2}>{detail}</Text>
          ) : null}
        </View>
        <View style={styles.presence}>
          <StatusDot status={dot} size={7} pulse={presence === 'connecting'} />
          {seen ? <Text style={[type.meta, { color: t.textSecondary }]} numberOfLines={1}>{seen}</Text> : null}
        </View>
      </Card>
    </Pressable>
  );
}

export default function DesktopsScreen() {
  const t = useTheme();
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
    if (d.id === activeId) {
      if (connection.status === 'connected') return 'online';
      if (connection.status === 'connecting' || connection.status === 'reconnecting') return 'connecting';
      return 'offline';
    }
    if (d.kind === 'remote') return remoteOnline[d.address] === true ? 'online' : remoteOnline[d.address] === false ? 'offline' : 'unknown';
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

  const androidActions = (d: LinkedDesktop) => {
    void Haptics.selectionAsync();
    Alert.alert(desktopDisplayName(d), undefined, [
      { text: 'Connect', onPress: () => void client.connect(d) },
      { text: 'Unlink', style: 'destructive', onPress: () => unlink(d) },
      { text: 'Cancel', style: 'cancel' },
    ]);
  };

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Desktops', headerLargeTitle: true, headerShown: true }} />
      {Platform.OS === 'ios' ? (
        <Stack.Toolbar placement="right">
          <Stack.Toolbar.Button icon="plus" accessibilityLabel="Pair phone" onPress={() => router.push('/pair')} />
        </Stack.Toolbar>
      ) : null}
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content}>
        {desktops.length === 0 ? (
          <EmptyState title="No desktops yet" actionLabel="Pair phone" onAction={() => router.push('/pair')} />
        ) : (
          <View style={styles.cards}>
            {ordered.map((d) => {
              const device = devices.find((x) => x.id === d.id || x.id === d.address)
                ?? (d.id === activeId ? devices.find((x) => x.id === connection.localDeviceId) : undefined);
              const card = (
                <DesktopCard
                  desktop={d}
                  device={device}
                  presence={presenceOf(d)}
                  active={d.id === activeId}
                  detail={connection.detail}
                  onPress={() => { void Haptics.selectionAsync(); if (Platform.OS === 'ios') void client.connect(d); else androidActions(d); }}
                />
              );
              if (Platform.OS !== 'ios') return <View key={d.id}>{card}</View>;
              return (
                <Link key={d.id} href="/(tabs)/(desktops)" asChild>
                  <Link.Trigger>{card}</Link.Trigger>
                  <Link.Menu title={desktopDisplayName(d)}>
                    <Link.MenuAction title="Connect" icon="link" onPress={() => void client.connect(d)} />
                    <Link.MenuAction title="Rename…" icon="pencil" onPress={() => rename(d)} />
                    <Link.MenuAction title="Unlink" icon="trash" destructive onPress={() => unlink(d)} />
                  </Link.Menu>
                </Link>
              );
            })}
            <Button title="Pair phone" onPress={() => router.push('/pair')} style={styles.pair} />
          </View>
        )}

        <SectionLabel>More ways to connect</SectionLabel>
        <Group>
          <Row
            title="On this network"
            subtitle="Link by address over one Wi-Fi"
            icon="wifi"
            onPress={() => router.push({ pathname: '/pair', params: { mode: 'local' } })}
            chevron
          />
          {!auth ? (
            <Row title="Sign in to Woozlit" subtitle="Reach your desktops from anywhere" icon="person.crop.circle" onPress={() => router.push('/sign-in')} chevron />
          ) : null}
        </Group>
        <Text style={[type.footnote, styles.footer, { color: t.textTertiary }]}>
          A desktop only accepts a phone it has paired with.
        </Text>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: edge, paddingBottom: spacing(12) },
  cards: { gap: spacing(2), paddingTop: spacing(2) },
  card: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), padding: spacing(3.5) },
  cardCopy: { flex: 1, minWidth: 0, gap: 2 },
  presence: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), maxWidth: 130 },
  pair: { marginTop: spacing(3) },
  footer: { marginTop: spacing(2), marginHorizontal: edge },
});
