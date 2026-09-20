import React from 'react';
import { Alert, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, Stack } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useCosmos, useSnapshot, LinkedDesktop } from '../../../lib/client';
import { useTheme, spacing } from '../../../lib/theme';
import { Row, Group, EmptyState, SectionLabel, StatusDot } from '../../../components/bits';
import { Screen } from '../../../components/surface';

function DesktopRow({ d, isActive, online, version }: {
  d: LinkedDesktop; isActive: boolean; online?: boolean; version?: string;
}) {
  const t = useTheme();
  const client = useCosmos();
  return (
    <Pressable
      onPress={() => void client.connect(d)}
      onLongPress={() => {
        Alert.alert(d.name, undefined, [
          { text: 'Connect', onPress: () => void client.connect(d) },
          {
            text: 'Rename',
            onPress: () => Alert.prompt?.('Rename desktop', undefined, (name) => {
              if (name?.trim()) void client.renameDesktop(d.id, name.trim());
            }),
          },
          {
            text: 'Unlink', style: 'destructive',
            onPress: () => Alert.alert('Unlink desktop?', 'Removes the pairing from this phone.', [
              { text: 'Cancel', style: 'cancel' },
              { text: 'Unlink', style: 'destructive', onPress: () => void client.removeDesktop(d.id) },
            ]),
          },
          { text: 'Cancel', style: 'cancel' },
        ]);
      }}
      style={({ pressed }) => [styles.dRow, pressed && { backgroundColor: t.fill }]}
    >
      <View style={[styles.dIcon, { backgroundColor: t.fill }]}>
        <SymbolView name="desktopcomputer" size={20} tintColor={t.textSecondary} />
      </View>
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={{ color: t.text, fontSize: 16, fontWeight: '500' }} numberOfLines={1}>{d.name}</Text>
        <Text style={{ color: t.textSecondary, fontSize: 13 }} numberOfLines={1}>
          {d.kind === 'local' ? `Local · ${d.address}` : 'Remote · via Woozlit'}
          {version ? ` · v${version}` : ''}
        </Text>
      </View>
      {isActive ? (
        <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <StatusDot status={online ? 'completed' : 'idle'} />
          <Text style={{ color: online ? t.success : t.textTertiary, fontSize: 12 }}>{online ? 'Online' : 'Offline'}</Text>
        </View>
      ) : (
        <Text style={{ color: t.link, fontSize: 13 }}>Connect</Text>
      )}
    </Pressable>
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

  const activeId = connection.desktop?.id;
  const engineDeviceId = connection.localDeviceId;
  const activeDevice = devices.find((d) => d.id === engineDeviceId);
  const sessionCount = sessions.length;

  return (
    <Screen>
      <Stack.Screen options={{ title: 'Desktops', headerLargeTitle: true, headerShown: true }} />
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={{ paddingHorizontal: spacing(4) }}>
        <SectionLabel>Linked</SectionLabel>
        {desktops.length === 0 ? (
          <Group>
            <EmptyState
              icon="desktopcomputer.trianglebadge.exclamationmark"
              title="No desktops linked"
              body="Link a Cosmos desktop on this network, or sign in to reach your desktops remotely."
              actionLabel="Link a desktop"
              onAction={() => router.push('/pair')}
            />
          </Group>
        ) : (
          <Group>
            {desktops.map((d) => (
              <DesktopRow
                key={d.id}
                d={d}
                isActive={d.id === activeId}
                online={d.id === activeId && connection.status === 'connected'}
                version={d.id === activeId ? activeDevice?.version : undefined}
              />
            ))}
          </Group>
        )}

        {connection.status === 'connected' ? (
          <>
            <SectionLabel>This desktop</SectionLabel>
            <Group>
              <Row title={activeDevice?.name ?? connection.desktop?.name ?? 'Desktop'} subtitle={activeDevice?.platform ?? undefined}
                leading={<SymbolView name="macpro.gen3" size={18} tintColor={t.textSecondary} />}
                trailing={<Text style={{ color: t.textTertiary, fontSize: 13 }}>{activeDevice?.version ? `v${activeDevice.version}` : ''}</Text>} />
              <Row title="Sessions" subtitle={`${sessionCount} live or recent`}
                leading={<SymbolView name="bubble.left.and.bubble.right" size={18} tintColor={t.textSecondary} />} />
              <Row
                title="Computer Use"
                subtitle={computer ? `Available — ${computer.control ?? computer.state ?? 'ready'}` : 'Not running on this desktop'}
                leading={<SymbolView name="display.and.arrow.down" size={18} tintColor={computer ? t.success : t.textTertiary} />}
              />
            </Group>
          </>
        ) : null}

        <SectionLabel>Add</SectionLabel>
        <Group>
          <Row
            title="Link a desktop on this network"
            subtitle="Direct connection — enter its IP address"
            leading={<SymbolView name="wifi" size={18} tintColor={t.textSecondary} />}
            onPress={() => router.push('/pair')}
            chevron
          />
          <Row
            title="Find remote desktops"
            subtitle={auth ? 'Desktops on your Woozlit account' : 'Sign in with Woozlit to reach your devices anywhere'}
            leading={<SymbolView name="globe" size={18} tintColor={t.textSecondary} />}
            onPress={() => router.push(auth ? '/pair' : '/sign-in')}
            chevron
          />
        </Group>
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  dRow: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(4), paddingVertical: spacing(3),
  },
  dIcon: { width: 38, height: 38, borderRadius: 10, alignItems: 'center', justifyContent: 'center' },
});
