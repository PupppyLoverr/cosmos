import React, { useState } from 'react';
import { ActivityIndicator, Alert, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { router } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useCosmos, useSnapshot } from '../lib/client';
import { useTheme, spacing, radii } from '../lib/theme';
import { Row, Group, SectionLabel } from '../components/bits';
import { localSpec, RpcClient } from '../lib/rpc';
import { EngineInfo } from '../lib/protocol';

export default function PairScreen() {
  const t = useTheme();
  const client = useCosmos();
  const auth = useSnapshot(client.auth);
  const devices = useSnapshot(client.devices);

  const [host, setHost] = useState('');
  const [name, setName] = useState('');
  const [probe, setProbe] = useState<{ state: 'idle' | 'probing' | 'ok' | 'fail'; info?: EngineInfo; error?: string }>({ state: 'idle' });
  const [busy, setBusy] = useState(false);

  // Remote: desktops already registered under this account appear via
  // WatchDevices once a remote connection exists; sign-in enables that path.
  const remoteCandidates = devices.filter((d) => d.id !== connectionLocalId(devices, client));

  function connectionLocalId(_ds: unknown, c: typeof client) {
    return c.connection.get().localDeviceId;
  }

  // The host field accepts `host` or `host:port` — the engine's IPC port
  // defaults to 27654 and only differs for side-by-side/dev instances.
  function hostParts(): { h: string; p: number } {
    const raw = host.trim();
    const m = /^(\S+?):(\d{1,5})$/.exec(raw);
    if (m) return { h: m[1], p: Math.min(65535, parseInt(m[2], 10)) };
    return { h: raw, p: 27654 };
  }

  const probeHost = async () => {
    if (!host.trim()) return;
    setProbe({ state: 'probing' });
    const { h, p } = hostParts();
    const rpc = new RpcClient(localSpec(h, p));
    try {
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('no reply')), 4000);
        rpc.onStatusChange((s) => {
          if (s === 'connected') { clearTimeout(timer); resolve(); }
        });
        rpc.connect();
      });
      const info = await rpc.call<EngineInfo>('EngineInfo').catch(() => null);
      const local = await rpc.call<{ deviceId: string }>('LocalDevice').catch(() => null);
      setProbe({ state: 'ok', info: { ...info, deviceId: local?.deviceId } as EngineInfo });
    } catch (e) {
      setProbe({ state: 'fail', error: e instanceof Error ? e.message : String(e) });
    } finally {
      rpc.close();
    }
  };

  const link = async () => {
    setBusy(true);
    try {
      const { h, p } = hostParts();
      const desktop = await client.addLocalDesktop(h, p, name || probe.info?.deviceId?.slice(0, 8));
      await client.connect(desktop);
      router.back();
    } catch (e) {
      Alert.alert('Could not link', e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const linkRemote = async (deviceId: string, deviceName: string) => {
    setBusy(true);
    try {
      const desktop = await client.addRemoteDesktop({ id: deviceId, name: deviceName, platform: 'remote' });
      await client.connect(desktop);
      router.back();
    } catch (e) {
      Alert.alert('Could not link', e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <View style={[styles.screen, { backgroundColor: t.background }]}>
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={{ padding: spacing(4) }}>
        <SectionLabel>Local network</SectionLabel>
        <Group>
          <View style={{ padding: spacing(3), gap: spacing(2.5) }}>
            <TextInput
              style={[styles.input, { color: t.text, borderColor: t.separator, backgroundColor: t.surfaceRaised }]}
              placeholder="Desktop IP, e.g. 192.168.1.20 or host:port"
              placeholderTextColor={t.textTertiary}
              autoCapitalize="none"
              autoCorrect={false}
              keyboardType="numbers-and-punctuation"
              value={host}
              onChangeText={setHost}
            />
            <TextInput
              style={[styles.input, { color: t.text, borderColor: t.separator, backgroundColor: t.surfaceRaised }]}
              placeholder="Name (optional)"
              placeholderTextColor={t.textTertiary}
              value={name}
              onChangeText={setName}
            />
            <Pressable onPress={() => void probeHost()} disabled={probe.state === 'probing' || !host.trim()}
              style={[styles.probeBtn, { borderColor: t.separator }]}>
              {probe.state === 'probing'
                ? <ActivityIndicator color={t.accent} size="small" />
                : <Text style={{ color: t.accent, fontWeight: '600' }}>Check connection</Text>}
            </Pressable>
            {probe.state === 'ok' ? (
              <View style={[styles.probeResult, { borderColor: t.success }]}>
                <SymbolView name="checkmark.circle.fill" size={16} tintColor={t.success} />
                <Text style={{ color: t.text, flex: 1 }}>
                  Cosmos desktop found{probe.info?.version ? ` · v${probe.info.version}` : ''}
                </Text>
              </View>
            ) : probe.state === 'fail' ? (
              <View style={[styles.probeResult, { borderColor: t.danger }]}>
                <SymbolView name="xmark.circle.fill" size={16} tintColor={t.danger} />
                <Text style={{ color: t.textSecondary, flex: 1 }}>No Cosmos engine at {hostParts().h}:{hostParts().p} — is the app running?</Text>
              </View>
            ) : null}
          </View>
        </Group>
        <Pressable
          onPress={() => void link()}
          disabled={probe.state !== 'ok' || busy}
          style={[styles.linkBtn, { backgroundColor: t.accent, opacity: probe.state === 'ok' && !busy ? 1 : 0.4 }]}
        >
          {busy ? <ActivityIndicator color={t.accentContrast} /> : <Text style={{ color: t.accentContrast, fontWeight: '600', fontSize: 16 }}>Trust & link</Text>}
        </Pressable>
        <Text style={{ color: t.textTertiary, fontSize: 12, textAlign: 'center', marginTop: spacing(2) }}>
          Only link desktops on networks you trust. Local pairing speaks the engine protocol directly — no cloud.
        </Text>

        {auth ? (
          <>
            <SectionLabel>Remote (Woozlit)</SectionLabel>
            {remoteCandidates.length === 0 ? (
              <Group>
                <Row title="No other devices online" subtitle="Desktops signed in to your account appear here when reachable."
                  leading={<SymbolView name="icloud.slash" size={18} tintColor={t.textTertiary} />} />
              </Group>
            ) : (
              <Group>
                {remoteCandidates.map((d) => (
                  <Row
                    key={d.id}
                    title={d.name}
                    subtitle={`${d.platform}${d.version ? ` · v${d.version}` : ''}${d.lastSeenAt ? ` · seen ${d.lastSeenAt}` : ''}`}
                    leading={<SymbolView name="desktopcomputer" size={18} tintColor={t.textSecondary} />}
                    trailing={busy ? <ActivityIndicator size="small" /> : <Text style={{ color: t.accent }}>Link</Text>}
                    onPress={() => void linkRemote(d.id, d.name)}
                  />
                ))}
              </Group>
            )}
          </>
        ) : (
          <>
            <SectionLabel>Remote (Woozlit)</SectionLabel>
            <Group>
              <Row title="Sign in to see remote desktops" leading={<SymbolView name="person.crop.circle" size={18} tintColor={t.accent} />}
                onPress={() => router.push('/sign-in')} chevron />
            </Group>
          </>
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  input: {
    borderWidth: StyleSheet.hairlineWidth, borderRadius: radii.md,
    paddingHorizontal: spacing(3), paddingVertical: spacing(2.5), fontSize: 15,
  },
  probeBtn: {
    borderWidth: 1, borderRadius: radii.md, alignItems: 'center', paddingVertical: spacing(2.5),
  },
  probeResult: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    borderWidth: 1, borderRadius: radii.md, padding: spacing(2.5),
  },
  linkBtn: { borderRadius: radii.xl, alignItems: 'center', paddingVertical: spacing(3.5), marginTop: spacing(4) },
});
