import React from 'react';
import { Alert, Platform, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, Stack } from 'expo-router';
import { Host, Switch } from '@expo/ui';
import Constants from 'expo-constants';
import { desktopDisplayName, useCosmos, useSnapshot } from '../../../lib/client';
import { alpha, useTheme, spacing, type, familyById, useThemePrefs, edge, radii, surfaceOptions, backdropPresets } from '../../../lib/theme';
import { Icon, Row, Group, StatusPill, Tag } from '../../../components/bits';
import { HarnessMark } from '../../../components/harness-mark';
import { Glass, Screen } from '../../../components/surface';
import { ensureNotificationPermission } from '../../../lib/notify';
import { markOnboardingUnseen } from '../../../lib/onboarding-store';

function connectionLabel(status: string, detail?: string) {
  if (status === 'connected') return { label: 'Online', color: 'success' as const };
  if (status === 'connecting') return { label: 'Connecting…', color: 'warning' as const };
  if (status === 'reconnecting') return { label: 'Reconnecting…', color: 'warning' as const };
  if (status === 'disconnected') return { label: 'Offline', color: 'tertiary' as const };
  return { label: detail ?? 'Error', color: 'danger' as const };
}

export default function SettingsScreen() {
  const t = useTheme();
  const client = useCosmos();
  const auth = useSnapshot(client.auth);
  const connection = useSnapshot(client.connection);
  const [notifEnabled, setNotifEnabled] = React.useState(false);
  const [prefs] = useThemePrefs();

  React.useEffect(() => {
    void ensureNotificationPermission().then(setNotifEnabled);
  }, []);

  const material = surfaceOptions.find((o) => o.id === prefs.surface)?.label ?? 'Auto';
  const b = prefs.backdrop;
  const backdrop = b.kind === 'photo'
    ? (prefs.effect === 'none' ? 'Photo' : `Photo · ${prefs.effect[0]!.toUpperCase()}${prefs.effect.slice(1)}`)
    : b.kind === 'preset'
      ? (backdropPresets.find((p) => p.id === b.id)?.name ?? 'Gradient')
      : 'None';
  const openAppearance = () => router.push('/(tabs)/(settings)/appearance' as never);
  const initials = (auth?.user.name || auth?.user.email || '?')
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]!.toUpperCase())
    .join('');
  const connectionStatus = connectionLabel(connection.status, connection.detail);

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Settings', headerShown: true }} />
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content}>
        {auth ? (
          <Glass radius={radii.xl} style={styles.accountCard}>
            <View style={[styles.avatar, { backgroundColor: alpha(t.accent, 0.16) }]}>
              <Text style={[type.headline, { color: t.link }]}>{initials}</Text>
            </View>
            <View style={styles.accountCopy}>
              <Text style={[type.headline, { color: t.text }]} numberOfLines={1}>{auth.user.name || auth.user.email}</Text>
              <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>{auth.user.email}</Text>
            </View>
            <Tag label="Woozlit" color={t.link} />
          </Glass>
        ) : (
          <Group>
            <Row
              tall
              title="Sign in with Woozlit"
              subtitle="Reach your desktops away from home"
              leading={<HarnessMark size={32} />}
              onPress={() => router.push('/sign-in')}
              chevron
            />
          </Group>
        )}

        <Group header="Desktop">
          <Row
            icon={connection.desktop?.kind === 'local' ? 'laptopcomputer' : 'antenna.radiowaves.left.and.right'}
            iconTint={connection.status === 'connected' ? t.success : t.textTertiary}
            title={connection.desktop ? desktopDisplayName(connection.desktop) : 'Not linked'}
            subtitle={connection.desktop ? (connection.desktop.kind === 'local' ? 'Local network' : 'Via Woozlit') : 'Link a desktop to get started'}
            trailing={<StatusPill status={connection.status} label={connectionStatus.label} />}
            onPress={() => router.push('/(tabs)/(desktops)' as never)}
            chevron
          />
        </Group>

        <Group header="Appearance">
          <Row
            title="Theme"
            value={familyById(prefs.family).name}
            leading={<View style={styles.appearanceLeading}><View style={[styles.swatch, { backgroundColor: t.accent }]} /><Icon name="paintpalette" size={16} color={t.textSecondary} /></View>}
            onPress={openAppearance}
            chevron
          />
          <Row title="Material" value={material} icon="square.on.square" onPress={openAppearance} chevron />
          <Row title="Backdrop" value={backdrop} icon="photo" onPress={openAppearance} chevron />
        </Group>

        <Group header="Notifications" footer="Get notified when a session finishes, needs your input, fails, or the desktop connection changes.">
          <Row
            icon="bell.badge"
            title="Session Alerts"
            trailing={(
              <Host matchContents colorScheme={t.scheme}>
                <Switch value={notifEnabled} onValueChange={(v) => { if (v) void ensureNotificationPermission().then(setNotifEnabled); else setNotifEnabled(false); }} />
              </Host>
            )}
          />
        </Group>

        {auth ? (
          <Group header="Account">
            <Row
              title="Sign Out"
              danger
              onPress={() => Alert.alert('Sign out?', 'Remote desktops need you signed in to pair.', [
                { text: 'Cancel', style: 'cancel' },
                { text: 'Sign Out', style: 'destructive', onPress: () => void client.signOut() },
              ])}
            />
          </Group>
        ) : null}

        <Group header="Help">
          <Row
            title="Show the welcome again"
            subtitle="Replay the first-run tour"
            icon="sparkles"
            onPress={() => { void markOnboardingUnseen().then(() => router.replace('/')); }}
            chevron
          />
        </Group>

        <View style={styles.about}>
          <Text style={[type.footnote, { color: t.textTertiary }]}>Cosmos for {Platform.OS === 'ios' ? 'iOS' : 'Android'}</Text>
          <Text style={[type.caption, { color: t.textTertiary }]}>Local-first companion for the Cosmos desktop.</Text>
          {Constants.expoConfig?.version ? <Text style={[type.caption, { color: t.textTertiary }]}>Version {Constants.expoConfig.version}</Text> : null}
        </View>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: edge, paddingBottom: spacing(12) },
  accountCard: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), padding: spacing(4) },
  avatar: { width: 48, height: 48, borderRadius: radii.pill, alignItems: 'center', justifyContent: 'center' },
  accountCopy: { flex: 1, minWidth: 0, gap: 2 },
  swatch: { width: 20, height: 20, borderRadius: radii.pill },
  appearanceLeading: { width: 50, flexDirection: 'row', alignItems: 'center', gap: spacing(2) },
  about: { alignItems: 'center', gap: 2, marginTop: spacing(10) },
});
