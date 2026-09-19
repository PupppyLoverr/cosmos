import React from 'react';
import { ScrollView, StyleSheet, Switch, Text, View } from 'react-native';
import { router, Stack } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useCosmos, useSnapshot } from '../../../lib/client';
import { useTheme, spacing, useAppearance, type } from '../../../lib/theme';
import { Row, Group, SectionLabel, Avatar, Segmented } from '../../../components/bits';
import { ensureNotificationPermission } from '../../../lib/notify';

export default function SettingsScreen() {
  const t = useTheme();
  const client = useCosmos();
  const auth = useSnapshot(client.auth);
  const connection = useSnapshot(client.connection);
  const [notifEnabled, setNotifEnabled] = React.useState(false);
  const [appearance, setAppearance] = useAppearance();

  React.useEffect(() => {
    void ensureNotificationPermission().then(setNotifEnabled);
  }, []);

  return (
    <View style={[styles.screen, { backgroundColor: t.background }]}>
      <Stack.Screen options={{ title: 'Settings', headerLargeTitle: true, headerShown: true }} />
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
          <View style={{ paddingHorizontal: spacing(4), paddingVertical: spacing(3), gap: spacing(3) }}>
            <Segmented
              value={appearance}
              onChange={setAppearance}
              options={[
                { id: 'system', label: 'System' },
                { id: 'light', label: 'Light' },
                { id: 'dark', label: 'Dark' },
              ]}
            />
          </View>
        </Group>

        <SectionLabel>Notifications</SectionLabel>
        <Group>
          <Row
            title="Session events"
            subtitle="Finished, needs input, failed, connection changes"
            leading={<SymbolView name="bell" size={18} tintColor={t.textSecondary} />}
            trailing={<Switch value={notifEnabled} onValueChange={(v) => { if (v) void ensureNotificationPermission().then(setNotifEnabled); else setNotifEnabled(false); }} />}
          />
        </Group>

        <SectionLabel>About</SectionLabel>
        <Group>
          <Row title="Cosmos for iOS" subtitle="Remote client for the Cosmos desktop" leading={<SymbolView name="info.circle" size={18} tintColor={t.textSecondary} />} />
        </Group>
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({ screen: { flex: 1 } });
