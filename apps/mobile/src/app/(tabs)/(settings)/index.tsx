import React from 'react';
import { Platform, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, Stack } from 'expo-router';
import { Host, Switch } from '@expo/ui';
import { desktopDisplayName, useCosmos, useSnapshot } from '../../../lib/client';
import { useTheme, spacing, type, familyById, useThemePrefs, edge, surfaceOptions, backdropPresets } from '../../../lib/theme';
import { Row, Group, ConnectionBadge } from '../../../components/bits';
import { HarnessMark } from '../../../components/harness-mark';
import { Screen } from '../../../components/surface';
import { ensureNotificationPermission } from '../../../lib/notify';

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

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Settings', headerShown: true }} />
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content}>
        <Group>
          {auth ? (
            <Row
              tall
              title={auth.user.name || auth.user.email}
              subtitle={auth.user.email}
              leading={<HarnessMark size={32} />}
            />
          ) : (
            <Row
              tall
              title="Sign in with Woozlit"
              subtitle="Reach your desktops away from home"
              leading={<HarnessMark size={32} />}
              onPress={() => router.push('/sign-in')}
              chevron
            />
          )}
        </Group>

        <Group header="Desktop">
          <Row
            icon={connection.desktop?.kind === 'local' ? 'laptopcomputer' : 'antenna.radiowaves.left.and.right'}
            iconTint={connection.status === 'connected' ? t.success : t.textTertiary}
            title={connection.desktop ? desktopDisplayName(connection.desktop) : 'Not linked'}
            subtitle={connection.desktop ? (connection.desktop.kind === 'local' ? 'Local network' : 'Via Woozlit') : 'Link a Mac to get started'}
            trailing={<ConnectionBadge status={connection.status} detail={connection.detail} />}
            onPress={() => router.push('/(tabs)/(desktops)' as never)}
            chevron
          />
        </Group>

        <Group header="Appearance">
          <Row title="Theme" value={familyById(prefs.family).name} onPress={openAppearance} chevron />
          <Row title="Material" value={material} onPress={openAppearance} chevron />
          <Row title="Backdrop" value={backdrop} onPress={openAppearance} chevron />
        </Group>

        <Group header="Notifications" footer="Get notified when a session finishes, needs your input, fails, or the desktop connection changes.">
          <Row
            icon="bell.badge"
            iconTint="#FF3B30"
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
            <Row title="Sign Out" danger onPress={() => void client.signOut()} />
          </Group>
        ) : null}

        <View style={styles.about}>
          <Text style={[type.footnote, { color: t.textTertiary }]}>Cosmos for {Platform.OS === 'ios' ? 'iOS' : 'Android'}</Text>
          <Text style={[type.caption, { color: t.textTertiary }]}>Local-first companion for the Cosmos desktop.</Text>
        </View>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: edge, paddingBottom: spacing(12) },
  about: { alignItems: 'center', gap: 2, marginTop: spacing(10) },
});
