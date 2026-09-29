import React from 'react';
import { Alert, Platform, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { Host, Switch } from '@expo/ui';
import Constants from 'expo-constants';
import { desktopDisplayName, useCosmos, useSnapshot } from '../../lib/client';
import { backdropPresets, edge, familyById, spacing, surfaceOptions, type, useTheme, useThemePrefs } from '../../lib/theme';
import { Group, Row, StatusDot } from '../../components/bits';
import { CircleButton, TopBar } from '../../components/chrome';
import { HarnessMark } from '../../components/harness-mark';
import { Screen } from '../../components/surface';
import { ensureNotificationPermission } from '../../lib/notify';
import { markOnboardingUnseen } from '../../lib/onboarding-store';
import { describeLink } from '../../lib/connection-state';


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
  const wallpaper = b.kind === 'photo'
    ? (prefs.effect === 'none' ? 'Photo' : `Photo · ${prefs.effect[0]!.toUpperCase()}${prefs.effect.slice(1)}`)
    : b.kind === 'preset' ? (backdropPresets.find((p) => p.id === b.id)?.name ?? 'Gradient') : 'None';
  const appearance = prefs.appearance === 'system' ? 'System' : prefs.appearance === 'dark' ? 'Dark' : 'Light';
  const openAppearance = () => router.push('/settings/appearance');
  const initials = (auth?.user.name || auth?.user.email || '?')
    .split(/\s+/).filter(Boolean).slice(0, 2).map((part) => part[0]!.toUpperCase()).join('');
  const link = describeLink(connection);
  const dot = link.tone === 'online' ? 'connected'
    : link.tone === 'pending' ? 'awaitingInput'
    : link.tone === 'attention' ? 'errored' : undefined;

  return (
    <Screen>
      <TopBar left={<CircleButton icon="chevron.left" label="Back" onPress={() => router.back()} />} title="Settings" center />
      <ScrollView contentContainerStyle={styles.content}>

        <Group>
          {auth ? (
            <Row
              tall
              title={auth.user.name || auth.user.email}
              subtitle={auth.user.name ? auth.user.email : 'Signed in with Woozlit'}
              leading={(
                <View style={[styles.avatar, { backgroundColor: t.surface }]}>
                  <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{initials}</Text>
                </View>
              )}
            />
          ) : (
            <Row
              tall
              title="Sign in with Woozlit"
              subtitle="Reach your desktops from anywhere"
              leading={<HarnessMark size={40} />}
              onPress={() => router.push('/sign-in')}
              chevron
            />
          )}
        </Group>

        <Group header="Desktop">
          <Row
            icon={connection.desktop?.kind === 'local' ? 'wifi' : 'globe'}
            title={connection.desktop ? desktopDisplayName(connection.desktop) : 'Not linked'}
            value={link.label}
            trailing={<StatusDot status={dot} size={8} />}
            onPress={() => router.push(link.needsPair ? '/pair' : '/desktops')}
            chevron
          />
          <Row icon="qrcode.viewfinder" title="Pair a desktop" onPress={() => router.push('/pair')} chevron />
        </Group>

        <Group header="Appearance">
          <Row title="Mode" value={appearance} icon="circle.lefthalf.filled" onPress={openAppearance} chevron />
          <Row title="Theme" value={familyById(prefs.family).name} icon="paintpalette" onPress={openAppearance} chevron />
          <Row title="Material" value={material} icon="square.on.square" onPress={openAppearance} chevron />
          <Row title="Wallpaper" value={wallpaper} icon="photo" onPress={openAppearance} chevron />
        </Group>

        <Group header="Notifications" footer="When a session finishes, needs you, or fails.">
          <Row
            icon="bell"
            title="Session alerts"
            trailing={(
              <Host matchContents colorScheme={t.scheme}>
                <Switch value={notifEnabled} onValueChange={(v) => { if (v) void ensureNotificationPermission().then(setNotifEnabled); else setNotifEnabled(false); }} />
              </Host>
            )}
          />
        </Group>

        <Group header="Help">
          <Row title="Show the welcome again" icon="arrow.counterclockwise" onPress={() => { void markOnboardingUnseen(); }} chevron />
        </Group>

        {auth ? (
          <Group style={styles.signOut}>
            <Row
              title="Sign out"
              danger
              onPress={() => Alert.alert('Sign out?', 'Remote desktops need you signed in to pair.', [
                { text: 'Cancel', style: 'cancel' },
                { text: 'Sign Out', style: 'destructive', onPress: () => void client.signOut() },
              ])}
            />
          </Group>
        ) : null}

        <Text style={[type.footnote, styles.about, { color: t.textTertiary }]}>
          Cosmos for {Platform.OS === 'ios' ? 'iOS' : 'Android'}{Constants.expoConfig?.version ? ` · ${Constants.expoConfig.version}` : ''}
        </Text>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: edge, paddingBottom: spacing(14) },
  title: { paddingHorizontal: 0, paddingTop: 0 },
  avatar: { width: 40, height: 40, borderRadius: 20, alignItems: 'center', justifyContent: 'center' },
  signOut: { marginTop: spacing(6) },
  about: { textAlign: 'center', marginTop: spacing(8) },
});
