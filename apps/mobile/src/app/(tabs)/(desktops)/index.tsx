import React, { useEffect } from 'react';
import { Alert, Platform, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { Link, router, Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { desktopDisplayName, useCosmos, useSnapshot, LinkedDesktop, type ConnectionInfo } from '../../../lib/client';
import { alpha, radii, spacing, type, useTheme, type Theme } from '../../../lib/theme';
import { EmptyState, Group, Icon, IconTile, Pulse, Row, SectionLabel, StatusPill, SuggestionCard, Tag } from '../../../components/bits';
import { Glass, Screen } from '../../../components/surface';

function connectionLabel(c: ConnectionInfo, t: Theme): { label: string; color: string } {
  switch (c.status) {
    case 'connected': return { label: 'Online', color: t.success };
    case 'connecting': return { label: 'Connecting…', color: t.warning };
    case 'reconnecting': return { label: 'Reconnecting…', color: t.warning };
    case 'disconnected': return { label: 'Offline', color: t.textTertiary };
    default: return { label: c.detail ?? 'Error', color: t.danger };
  }
}

function platformIcon(platform?: string): string {
  const p = (platform ?? '').toLowerCase();
  if (p.includes('mac') || p.includes('darwin')) return 'laptopcomputer';
  if (p.includes('win')) return 'pc';
  if (p.includes('linux')) return 'server.rack';
  return 'desktopcomputer';
}

/** The connected desktop: one hero card with live facts instead of a list of rows. */
function ActiveDesktop({ desktop, connection, version, platform, sessions, computer }: {
  desktop: LinkedDesktop; connection: ConnectionInfo; version?: string; platform?: string; sessions: number; computer?: string;
}) {
  const t = useTheme();
  const c = connectionLabel(connection, t);
  return (
    <Glass radius={radii.xl} style={styles.hero}>
      <View style={styles.heroTop}>
        <View style={[styles.heroIcon, { backgroundColor: alpha(t.accent, 0.16) }]}>
          <Icon name={platformIcon(platform)} size={26} color={t.link} weight="regular" />
        </View>
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text style={[type.title3, { color: t.text }]} numberOfLines={1}>{desktopDisplayName(desktop)}</Text>
          <View style={styles.status}>
            <StatusPill status={connection.status} label={c.label} pulse={connection.status === 'connecting' || connection.status === 'reconnecting'} size="md" />
            {version ? <Text style={[type.subhead, { color: t.textTertiary }]}>· Cosmos {version}</Text> : null}
          </View>
        </View>
      </View>
      <View style={[styles.facts, { borderTopColor: t.separator }]}>
        <Fact icon={desktop.kind === 'local' ? 'wifi' : 'globe'} label={desktop.kind === 'local' ? 'Local network' : 'Remote'} value={desktop.kind === 'local' ? desktop.address : 'via Woozlit'} />
        <View style={[styles.factSep, { backgroundColor: t.separator }]} />
        <Fact icon="bubble.left.and.bubble.right" label="Sessions" value={`${sessions} live`} onPress={() => router.push('/(tabs)/(sessions)')} />
        <View style={[styles.factSep, { backgroundColor: t.separator }]} />
        <Fact icon="display" label="Computer Use" value={computer ?? 'Idle'} accent={!!computer} pulse={!!computer} />
      </View>
    </Glass>
  );
}

function Fact({ icon, label, value, accent, pulse, onPress }: { icon: string; label: string; value: string; accent?: boolean; pulse?: boolean; onPress?: () => void }) {
  const t = useTheme();
  const content = (
    <>
      {pulse ? <Pulse color={t.link} /> : <Icon name={icon} size={15} color={accent ? t.link : t.textSecondary} />}
      <Text style={[type.meta, { color: t.textTertiary, marginTop: spacing(1.5) }]}>{label}</Text>
      <Text style={[type.footnote, { color: accent ? t.link : t.text, fontWeight: '600' }]} numberOfLines={1}>{value}</Text>
    </>
  );
  if (onPress) {
    return <Pressable style={styles.fact} onPress={onPress} accessibilityRole="button" accessibilityLabel={`${label}, ${value}`}>{content}</Pressable>;
  }
  return <View style={styles.fact}>{content}</View>;
}

export default function DesktopsScreen() {
  const t = useTheme();
  const client = useCosmos();
  const desktops = useSnapshot(client.desktops);
  const connection = useSnapshot(client.connection);
  const devices = useSnapshot(client.devices);
  const sessions = useSnapshot(client.sessions);
  const computer = useSnapshot(client.computer);
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

  const active = desktops.find((d) => d.id === connection.desktop?.id);
  const others = desktops.filter((d) => d.id !== active?.id);
  const activeDevice = devices.find((d) => d.id === connection.localDeviceId);

  const unlink = (d: LinkedDesktop) => {
    Alert.alert('Unlink this desktop?', 'Removes the pairing from this phone. The desktop keeps its own settings.', [
      { text: 'Cancel', style: 'cancel' },
      { text: 'Unlink', style: 'destructive', onPress: () => void client.removeDesktop(d.id) },
    ]);
  };

  const actions = (d: LinkedDesktop) => {
    void Haptics.selectionAsync();
    const items: { label: string; onPress: () => void; destructive?: boolean }[] = [
      { label: 'Connect', onPress: () => void client.connect(d) },
      ...(Platform.OS === 'ios' ? [{
        label: 'Rename…',
        onPress: () => Alert.prompt('Rename desktop', undefined, (name) => { if (name?.trim()) void client.renameDesktop(d.id, name.trim()); }, 'plain-text', d.name),
      }] : []),
      {
        label: 'Unlink', destructive: true,
        onPress: () => unlink(d),
      },
    ];
    if (Platform.OS !== 'ios') {
      Alert.alert(d.name, undefined, [
        ...items.map((i) => ({ text: i.label, onPress: i.onPress, style: i.destructive ? ('destructive' as const) : ('default' as const) })),
        { text: 'Cancel', style: 'cancel' as const },
      ]);
    }
  };

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Desktops', headerLargeTitle: true, headerShown: true }} />
      {Platform.OS === 'ios' ? (
        <Stack.Toolbar placement="right">
          <Stack.Toolbar.Button icon="plus" accessibilityLabel="Link a desktop" onPress={() => router.push('/pair')} />
        </Stack.Toolbar>
      ) : null}
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content}>
        {desktops.length === 0 ? (
          <Glass radius={radii.xl}>
            <EmptyState
              icon="desktopcomputer.and.arrow.down"
              title="No desktops yet"
              body="Link Cosmos running on your desktop over this network, or sign in to reach it from anywhere."
              actionLabel="Link a Desktop"
              onAction={() => router.push('/pair')}
            >
              <View style={styles.suggestions}>
                <SuggestionCard icon="qrcode.viewfinder" title="Scan the desktop QR" onPress={() => router.push('/pair')} />
                {!auth ? <SuggestionCard icon="person.crop.circle" title="Sign in to Woozlit" onPress={() => router.push('/sign-in')} /> : null}
              </View>
            </EmptyState>
          </Glass>
        ) : null}

        {active ? (
          <Link href="/(tabs)/(desktops)" asChild>
            <Link.Trigger>
              <Pressable onLongPress={Platform.OS !== 'ios' ? () => actions(active) : undefined} accessibilityLabel={`${active.name}, ${connectionLabel(connection, t).label}`} accessibilityHint="Long press for options">
                <ActiveDesktop
                  desktop={active}
                  connection={connection}
                  version={activeDevice?.version}
                  platform={activeDevice?.platform}
                  sessions={sessions.length}
                  computer={computer ? (computer.control ?? computer.state ?? 'Active') : undefined}
                />
              </Pressable>
            </Link.Trigger>
            <Link.Menu title={desktopDisplayName(active)}>
              <Link.MenuAction title="Connect" icon="link" onPress={() => void client.connect(active)} />
              <Link.MenuAction title="Rename…" icon="pencil" hidden={Platform.OS !== 'ios'} onPress={() => Alert.prompt('Rename desktop', undefined, (name) => { if (name?.trim()) void client.renameDesktop(active.id, name.trim()); }, 'plain-text', active.name)} />
              <Link.MenuAction title="Unlink" icon="trash" destructive onPress={() => unlink(active)} />
            </Link.Menu>
          </Link>
        ) : null}

        {others.length ? (
          <>
            <SectionLabel>Other desktops</SectionLabel>
            <Group>
              {others.map((d) => {
                const device = devices.find((item) => item.id === d.id);
                const online = d.kind === 'remote' ? remoteOnline[d.address] === true : undefined;
                return (
                  <Link key={d.id} href="/(tabs)/(desktops)" asChild>
                    <Link.Trigger>
                      <Row
                        tall
                        title={desktopDisplayName(d)}
                        subtitle={d.kind === 'local' ? `Local · ${d.address}` : `Remote · ${remoteOnline[d.address] === true ? 'Online' : remoteOnline[d.address] === false ? 'Offline' : 'via Woozlit'}`}
                        leading={<IconTile name={device ? platformIcon(device.platform) : d.kind === 'local' ? 'laptopcomputer' : 'globe'} tint={t.textTertiary} size={32} />}
                        trailing={d.kind === 'remote' ? <StatusPill status={online ? 'connected' : 'disconnected'} label={online ? 'Online' : 'Offline'} /> : <Tag label="Local" />}
                        chevron
                        onPress={() => { void Haptics.selectionAsync(); void client.connect(d); }}
                        onLongPress={Platform.OS !== 'ios' ? () => actions(d) : undefined}
                      />
                    </Link.Trigger>
                    <Link.Menu title={desktopDisplayName(d)}>
                      <Link.MenuAction title="Connect" icon="link" onPress={() => void client.connect(d)} />
                      <Link.MenuAction title="Rename…" icon="pencil" hidden={Platform.OS !== 'ios'} onPress={() => Alert.prompt('Rename desktop', undefined, (name) => { if (name?.trim()) void client.renameDesktop(d.id, name.trim()); }, 'plain-text', d.name)} />
                      <Link.MenuAction title="Unlink" icon="trash" destructive onPress={() => unlink(d)} />
                    </Link.Menu>
                  </Link>
                );
              })}
            </Group>
          </>
        ) : null}

        <SectionLabel>Add</SectionLabel>
        <Group footer="A desktop only accepts a phone it has trusted. On the same network, link it by address; from anywhere else, sign in and scan the code shown in Cosmos Settings › Desktops.">
          <Row title="Link a desktop" subtitle="Scan the code or enter a same-network address" icon="link" onPress={() => router.push('/pair')} chevron />
          {!auth ? <Row title="Sign in to Woozlit" subtitle="Reach your desktops anywhere" icon="globe" onPress={() => router.push('/sign-in')} chevron /> : null}
        </Group>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: spacing(4), paddingBottom: spacing(10) },
  hero: { padding: spacing(4), marginTop: spacing(2) },
  heroTop: { flexDirection: 'row', alignItems: 'center', gap: spacing(3.5) },
  heroIcon: { width: 48, height: 48, borderRadius: radii.md, alignItems: 'center', justifyContent: 'center' },
  status: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), marginTop: 2 },
  facts: { flexDirection: 'row', marginTop: spacing(4), paddingTop: spacing(3.5), borderTopWidth: StyleSheet.hairlineWidth },
  fact: { flex: 1, alignItems: 'flex-start', paddingHorizontal: spacing(1), minWidth: 0 },
  factSep: { width: StyleSheet.hairlineWidth, alignSelf: 'stretch', marginHorizontal: spacing(2) },
  suggestions: { width: '100%', gap: spacing(2), marginTop: spacing(5) },
});
