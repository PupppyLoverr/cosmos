import React from 'react';
import { ActionSheetIOS, Alert, Platform, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, Stack } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { useCosmos, useSnapshot, LinkedDesktop, type ConnectionInfo } from '../../../lib/client';
import { alpha, radii, spacing, type, useTheme, type Theme } from '../../../lib/theme';
import { Row, Group, EmptyState, SectionLabel, Icon, IconTile, Tag } from '../../../components/bits';
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
          <Icon name={platformIcon(platform)} size={30} color={t.link} weight="regular" />
        </View>
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text style={[type.title3, { color: t.text }]} numberOfLines={1}>{desktop.name}</Text>
          <View style={styles.status}>
            <View style={[styles.dot, { backgroundColor: c.color }]} />
            <Text style={[type.subhead, { color: c.color, fontWeight: '600' }]}>{c.label}</Text>
            {version ? <Text style={[type.subhead, { color: t.textTertiary }]}>· Cosmos {version}</Text> : null}
          </View>
        </View>
      </View>
      <View style={[styles.facts, { borderTopColor: t.separator }]}>
        <Fact icon={desktop.kind === 'local' ? 'wifi' : 'globe'} label={desktop.kind === 'local' ? 'Local network' : 'Remote'} value={desktop.kind === 'local' ? desktop.address : 'via Woozlit'} />
        <View style={[styles.factSep, { backgroundColor: t.separator }]} />
        <Fact icon="bubble.left.and.bubble.right" label="Sessions" value={`${sessions} live`} />
        <View style={[styles.factSep, { backgroundColor: t.separator }]} />
        <Fact icon="display" label="Computer Use" value={computer ?? 'Idle'} accent={!!computer} />
      </View>
    </Glass>
  );
}

function Fact({ icon, label, value, accent }: { icon: string; label: string; value: string; accent?: boolean }) {
  const t = useTheme();
  return (
    <View style={styles.fact}>
      <Icon name={icon} size={15} color={accent ? t.link : t.textSecondary} />
      <Text style={[type.caption2, { color: t.textTertiary, marginTop: spacing(1.5) }]}>{label.toUpperCase()}</Text>
      <Text style={[type.footnote, { color: accent ? t.link : t.text, fontWeight: '600' }]} numberOfLines={1}>{value}</Text>
    </View>
  );
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

  const active = desktops.find((d) => d.id === connection.desktop?.id);
  const others = desktops.filter((d) => d.id !== active?.id);
  const activeDevice = devices.find((d) => d.id === connection.localDeviceId);

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
        onPress: () => Alert.alert('Unlink this desktop?', 'Removes the pairing from this phone. The desktop keeps its own settings.', [
          { text: 'Cancel', style: 'cancel' },
          { text: 'Unlink', style: 'destructive', onPress: () => void client.removeDesktop(d.id) },
        ]),
      },
    ];
    if (Platform.OS === 'ios') {
      ActionSheetIOS.showActionSheetWithOptions(
        { title: d.name, options: [...items.map((i) => i.label), 'Cancel'], cancelButtonIndex: items.length, destructiveButtonIndex: items.length - 1 },
        (i) => items[i]?.onPress(),
      );
    } else {
      Alert.alert(d.name, undefined, [
        ...items.map((i) => ({ text: i.label, onPress: i.onPress, style: i.destructive ? ('destructive' as const) : ('default' as const) })),
        { text: 'Cancel', style: 'cancel' as const },
      ]);
    }
  };

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Desktops', headerLargeTitle: true, headerShown: true }} />
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content}>
        {desktops.length === 0 ? (
          <Glass radius={radii.xl}>
            <EmptyState
              icon="desktopcomputer.and.arrow.down"
              title="No desktops yet"
              body="Link the Cosmos app on your Mac over this network, or sign in to reach your desktops from anywhere."
              actionLabel="Link a Desktop"
              onAction={() => router.push('/pair')}
            />
          </Glass>
        ) : null}

        {active ? (
          <Pressable onLongPress={() => actions(active)} accessibilityLabel={`${active.name}, ${connectionLabel(connection, t).label}`} accessibilityHint="Long press for options">
            <ActiveDesktop
              desktop={active}
              connection={connection}
              version={activeDevice?.version}
              platform={activeDevice?.platform}
              sessions={sessions.length}
              computer={computer ? (computer.control ?? computer.state ?? 'Active') : undefined}
            />
          </Pressable>
        ) : null}

        {others.length ? (
          <>
            <SectionLabel>Other desktops</SectionLabel>
            <Group>
              {others.map((d) => (
                <Row
                  key={d.id}
                  tall
                  title={d.name}
                  subtitle={d.kind === 'local' ? `Local · ${d.address}` : 'Remote · via Woozlit'}
                  leading={<IconTile name={d.kind === 'local' ? 'laptopcomputer' : 'globe'} tint={t.textTertiary} size={32} />}
                  trailing={<Tag label="Connect" color={t.link} />}
                  onPress={() => { void Haptics.selectionAsync(); void client.connect(d); }}
                  onLongPress={() => actions(d)}
                />
              ))}
            </Group>
          </>
        ) : null}

        <SectionLabel>Add</SectionLabel>
        <Group footer="A desktop only accepts a phone after you approve it on the Mac. Remote access is roadmap — today Cosmos pairs over your local network.">
          <Row
            title="Link a desktop on this network"
            subtitle="Enter its address, then trust it on the Mac"
            icon="wifi"
            iconTint={t.link}
            onPress={() => router.push('/pair')}
            chevron
          />
          <Row
            title={auth ? 'Find remote desktops' : 'Sign in to Woozlit'}
            subtitle={auth ? 'Desktops signed into your account' : 'Reach your desktops anywhere'}
            icon="globe"
            iconTint={t.success}
            onPress={() => router.push(auth ? '/pair' : '/sign-in')}
            chevron
          />
        </Group>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: spacing(4), paddingBottom: spacing(10) },
  hero: { padding: spacing(4), marginTop: spacing(2) },
  heroTop: { flexDirection: 'row', alignItems: 'center', gap: spacing(3.5) },
  heroIcon: { width: 56, height: 56, borderRadius: 16, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  status: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), marginTop: 2 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  facts: { flexDirection: 'row', marginTop: spacing(4), paddingTop: spacing(3.5), borderTopWidth: StyleSheet.hairlineWidth },
  fact: { flex: 1, alignItems: 'flex-start', paddingHorizontal: spacing(1) },
  factSep: { width: StyleSheet.hairlineWidth, alignSelf: 'stretch', marginHorizontal: spacing(2) },
});
