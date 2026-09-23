import React from 'react';
import { Alert, Platform, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, Stack } from 'expo-router';
import { Host, Switch } from '@expo/ui';
import Constants from 'expo-constants';
import { desktopDisplayName, useCosmos, useSnapshot } from '../../../lib/client';
import { useTheme, spacing, type, familyById, useThemePrefs, edge, surfaceOptions, backdropPresets } from '../../../lib/theme';
import { Row, Group, StatusDot } from '../../../components/bits';
import { HarnessMark } from '../../../components/harness-mark';
import { Screen } from '../../../components/surface';
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
        <Group style={styles.first}>
          {auth ? (
            <Row
              tall
              title={auth.user.name || auth.user.email}
              subtitle={auth.user.name ? `${auth.user.email} · Woozlit` : 'Woozlit'}
              leading={(
                <View style={[styles.avatar, { backgroundColor: t.fill }]}>
                  <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{initials}</Text>
                </View>
              )}
            />
          ) : (
            <Row
              tall
              title="Sign in with Woozlit"
              subtitle="Reach your desktops from anywhere"
              leading={<HarnessMark size={36} />}
              onPress={() => router.push('/sign-in')}
              chevron
            />
          )}
        </Group>

        <Group header="Desktop">
          <Row
            icon={connection.desktop?.kind === 'local' ? 'wifi' : 'globe'}
            title={connection.desktop ? desktopDisplayName(connection.desktop) : 'Not linked'}
            value={connectionStatus.label}
            trailing={<StatusDot status={connection.status === 'connected' ? 'connected' : connection.status === 'connecting' || connection.status === 'reconnecting' ? 'awaitingInput' : undefined} size={7} />}
            onPress={() => router.push('/(tabs)/(desktops)' as never)}
            chevron
          />
        </Group>

        <Group header="Appearance">
          <Row title="Theme" value={familyById(prefs.family).name} icon="paintpalette" onPress={openAppearance} chevron />
          <Row title="Material" value={material} icon="square.on.square" onPress={openAppearance} chevron />
          <Row title="Wallpaper" value={backdrop} icon="photo" onPress={openAppearance} chevron />
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
            icon="arrow.counterclockwise"
            onPress={() => { void markOnboardingUnseen(); }}
            chevron
          />
        </Group>

        <View style={styles.about}>
          <Text style={[type.footnote, { color: t.textTertiary }]}>
            Cosmos for {Platform.OS === 'ios' ? 'iOS' : 'Android'}{Constants.expoConfig?.version ? ` · ${Constants.expoConfig.version}` : ''}
          </Text>
        </View>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: edge, paddingBottom: spacing(12) },
  first: { marginTop: spacing(2) },
  avatar: { width: 36, height: 36, borderRadius: 18, alignItems: 'center', justifyContent: 'center' },
  about: { alignItems: 'center', marginTop: spacing(10) },
});
