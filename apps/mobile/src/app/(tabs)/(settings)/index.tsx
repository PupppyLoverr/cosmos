import React from 'react';
import { Platform, ScrollView, Text, View } from 'react-native';
import { router, Stack } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { Host, Switch } from '@expo/ui';
import { useCosmos, useSnapshot } from '../../../lib/client';
import { useTheme, spacing, type, familyById, useThemePrefs } from '../../../lib/theme';
import { Row, Group, SectionLabel, Avatar } from '../../../components/bits';
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

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Settings', headerShown: true }} />
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={{ paddingHorizontal: spacing(4), paddingBottom: spacing(10) }}>

        <SectionLabel>Account</SectionLabel>
        <Group>
          {auth ? (
            <>
              <Row
                title={auth.user.name || auth.user.email}
                subtitle={auth.user.email}
                leading={<Avatar name={auth.user.name || auth.user.email} />}
              />
              <Row
                title="Sign out"
                danger
                leading={<SymbolView name="rectangle.portrait.and.arrow.right" size={18} tintColor={t.danger} />}
                onPress={() => void client.signOut()}
              />
            </>
          ) : (
            <Row
              title="Sign in with Woozlit"
              subtitle="Required for remote desktops"
              leading={<SymbolView name="person.crop.circle" size={20} tintColor={t.link} />}
              onPress={() => router.push('/sign-in')}
              chevron
            />
          )}
        </Group>

        <SectionLabel>Connection</SectionLabel>
        <Group>
          <Row
            title={connection.desktop?.name ?? 'No desktop'}
            subtitle={connection.status === 'connected'
              ? `${connection.desktop?.kind === 'local' ? 'Local' : 'Remote'} · connected`
              : connection.status}
            leading={<SymbolView name="network" size={18} tintColor={t.textSecondary} />}
            trailing={<Text style={[type.footnote, { color: t.textTertiary }]}>{connection.desktop?.kind === 'local' ? 'Direct' : 'Relay'}</Text>}
          />
          <Row
            title="Switch desktop"
            leading={<SymbolView name="arrow.triangle.swap" size={18} tintColor={t.textSecondary} />}
            onPress={() => router.push('/(tabs)/(desktops)' as never)}
            chevron
          />
        </Group>

        <SectionLabel>Appearance</SectionLabel>
        <Group>
          <Row
            title="Theme & background"
            subtitle={`${familyById(prefs.family).name} · ${prefs.appearance === 'system' ? 'System' : prefs.appearance === 'dark' ? 'Dark' : 'Light'}${prefs.backdrop.kind !== 'none' ? ' · Background' : ''}`}
            leading={<SymbolView name="paintpalette" size={18} tintColor={t.link} />}
            trailing={<View style={{ width: 18, height: 18, borderRadius: 9, backgroundColor: t.accent }} />}
            onPress={() => router.push('/(tabs)/(settings)/appearance' as never)}
            chevron
          />
        </Group>

        <SectionLabel>Notifications</SectionLabel>
        <Group>
          <Row
            title="Session events"
            subtitle="Finished, needs input, failed, connection changes"
            leading={<SymbolView name="bell" size={18} tintColor={t.textSecondary} />}
            trailing={(
              <Host matchContents colorScheme={t.scheme}>
                <Switch value={notifEnabled} onValueChange={(v) => { if (v) void ensureNotificationPermission().then(setNotifEnabled); else setNotifEnabled(false); }} />
              </Host>
            )}
          />
        </Group>

        <SectionLabel>About</SectionLabel>
        <Group>
          <Row title={`Cosmos for ${Platform.OS === 'ios' ? 'iOS' : 'Android'}`} subtitle="Remote client for the Cosmos desktop" leading={<SymbolView name="info.circle" size={18} tintColor={t.textSecondary} />} />
        </Group>
      </ScrollView>
    </Screen>
  );
}
