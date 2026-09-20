import React from 'react';
import { Platform, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, Stack } from 'expo-router';
import { Host, Switch } from '@expo/ui';
import { useCosmos, useSnapshot } from '../../../lib/client';
import { useTheme, spacing, type, familyById, useThemePrefs, edge, surfaceOptions } from '../../../lib/theme';
import { Row, Group, Icon, ConnectionBadge } from '../../../components/bits';
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

  const appearanceSummary = [
    familyById(prefs.family).name,
    surfaceOptions.find((o) => o.id === prefs.surface)?.label,
    prefs.backdrop.kind === 'photo' ? 'Photo' : prefs.backdrop.kind === 'preset' ? 'Gradient' : null,
  ].filter(Boolean).join(' · ');

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
              leading={<Monogram name={auth.user.name || auth.user.email} />}
            />
          ) : (
            <Row
              tall
              title="Sign in with Woozlit"
              subtitle="Reach your desktops away from home"
              leading={<Monogram />}
              onPress={() => router.push('/sign-in')}
              chevron
            />
          )}
        </Group>

        <Group header="Desktop">
          <Row
            icon={connection.desktop?.kind === 'local' ? 'laptopcomputer' : 'antenna.radiowaves.left.and.right'}
            iconTint={connection.status === 'connected' ? t.success : t.textTertiary}
            title={connection.desktop?.name ?? 'Not linked'}
            subtitle={connection.desktop ? (connection.desktop.kind === 'local' ? 'Local network' : 'Via Woozlit') : 'Link a Mac to get started'}
            trailing={<ConnectionBadge status={connection.status} detail={connection.detail} />}
            onPress={() => router.push('/(tabs)/(desktops)' as never)}
            chevron
          />
        </Group>

        <Group header="Personalization">
          <Row
            icon="paintpalette"
            iconTint={t.accentStrong}
            title="Appearance"
            value={appearanceSummary}
            onPress={() => router.push('/(tabs)/(settings)/appearance' as never)}
            chevron
          />
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

function Monogram({ name }: { name?: string }) {
  const t = useTheme();
  const initial = name?.trim().charAt(0).toUpperCase();
  return (
    <View style={[styles.monogram, { backgroundColor: initial ? t.accentStrong : t.fill }]}>
      {initial ? <Text style={[type.title3, { color: t.accentContrast }]}>{initial}</Text> : <Icon name="person.crop.circle.fill" size={26} color={t.textSecondary} />}
    </View>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: edge, paddingBottom: spacing(12) },
  monogram: { width: 48, height: 48, borderRadius: 24, alignItems: 'center', justifyContent: 'center' },
  about: { alignItems: 'center', gap: 2, marginTop: spacing(10) },
});
