import React, { useState } from 'react';
import { ActivityIndicator, Alert, KeyboardAvoidingView, Platform, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { PLACEHOLDER_NAME, useCosmos, useSnapshot } from '../lib/client';
import { alpha, radii, spacing, type, useTheme } from '../lib/theme';
import { Button, Field, Group, Icon, IconTile, Row, SectionLabel } from '../components/bits';
import { localSpec, RpcClient } from '../lib/rpc';
import { EngineInfo } from '../lib/protocol';
import { Screen } from '../components/surface';

type Probe = { state: 'idle' | 'probing' | 'ok' | 'fail'; info?: EngineInfo; error?: string };

const DEFAULT_PORT = 27654;

// Accepts `host` or `host:port`; the engine's IPC port only differs for side-by-side/dev instances.
function hostParts(raw: string): { h: string; p: number } {
  const s = raw.trim();
  const m = /^(\S+?):(\d{1,5})$/.exec(s);
  if (m) return { h: m[1], p: Math.min(65535, parseInt(m[2], 10)) };
  return { h: s, p: DEFAULT_PORT };
}

function Step({ n, title, body, done }: { n: number; title: string; body: string; done?: boolean }) {
  const t = useTheme();
  return (
    <View style={styles.step}>
      <View style={[styles.stepBadge, { backgroundColor: done ? t.success : t.accentMuted }]}>
        {done
          ? <Icon name="checkmark" size={12} color="#fff" weight="bold" />
          : <Text style={[type.caption, { color: t.link, fontWeight: '700' }]}>{n}</Text>}
      </View>
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{title}</Text>
        <Text style={[type.footnote, { color: t.textSecondary }]}>{body}</Text>
      </View>
    </View>
  );
}

export default function PairScreen() {
  const t = useTheme();
  const client = useCosmos();
  const auth = useSnapshot(client.auth);
  const devices = useSnapshot(client.devices);
  const connection = useSnapshot(client.connection);

  const [host, setHost] = useState('');
  const [name, setName] = useState('');
  const [probe, setProbe] = useState<Probe>({ state: 'idle' });
  const [busy, setBusy] = useState(false);

  const remoteCandidates = devices.filter((d) => d.id !== connection.localDeviceId);
  const target = hostParts(host);

  const onHost = (v: string) => { setHost(v); if (probe.state !== 'idle') setProbe({ state: 'idle' }); };

  const probeHost = async () => {
    if (!host.trim()) return;
    setProbe({ state: 'probing' });
    const { h, p } = hostParts(host);
    const rpc = new RpcClient(localSpec(h, p));
    try {
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('no reply')), 4000);
        rpc.onStatusChange((s) => { if (s === 'connected') { clearTimeout(timer); resolve(); } });
        rpc.connect();
      });
      const info = await rpc.call<EngineInfo>('EngineInfo').catch(() => null);
      const local = await rpc.call<{ deviceId: string }>('LocalDevice').catch(() => null);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      setProbe({ state: 'ok', info: { ...info, deviceId: local?.deviceId } as EngineInfo });
    } catch (e) {
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error);
      setProbe({ state: 'fail', error: e instanceof Error ? e.message : String(e) });
    } finally {
      rpc.close();
    }
  };

  const leave = () => { if (router.canGoBack()) router.back(); else router.replace('/'); };

  const link = async () => {
    setBusy(true);
    try {
      const { h, p } = hostParts(host);
      const desktop = await client.addLocalDesktop(h, p, name.trim() || PLACEHOLDER_NAME);
      await client.connect(desktop);
      leave();
    } catch (e) {
      Alert.alert("Couldn't link", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const linkRemote = async (deviceId: string, deviceName: string) => {
    setBusy(true);
    try {
      const desktop = await client.addRemoteDesktop({ id: deviceId, name: deviceName, platform: 'remote' });
      await client.connect(desktop);
      leave();
    } catch (e) {
      Alert.alert("Couldn't link", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const found = probe.state === 'ok';

  return (
    <Screen>
      <KeyboardAvoidingView style={{ flex: 1 }} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
        <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content} keyboardDismissMode="interactive" keyboardShouldPersistTaps="handled">
          <View style={styles.steps}>
            <Step n={1} title="Open Cosmos on your Mac" body="Both devices need to be on the same Wi‑Fi." done={found} />
            <Step n={2} title="Enter its address" body="Settings › Devices on the Mac shows the IP." done={found} />
            <Step n={3} title="Approve on the Mac" body="The desktop asks before this phone gets control." />
          </View>

          <SectionLabel>Local network</SectionLabel>
          <Group footer={found ? undefined : `Port ${DEFAULT_PORT} is used unless you specify host:port.`}>
            <Field
              label="Address"
              placeholder="192.168.1.20"
              value={host}
              onChangeText={onHost}
              autoCapitalize="none"
              autoCorrect={false}
              keyboardType="numbers-and-punctuation"
              returnKeyType="go"
              onSubmitEditing={() => void probeHost()}
              autoFocus
            />
            <View style={[styles.sep, { backgroundColor: t.separator }]} />
            <Field label="Name" placeholder="Uses the Mac's name" value={name} onChangeText={setName} returnKeyType="done" />
          </Group>

          {probe.state === 'ok' ? (
            <View style={[styles.result, { backgroundColor: alpha(t.success, 0.12) }]} accessibilityLiveRegion="polite">
              <Icon name="checkmark.seal.fill" size={20} color={t.success} />
              <View style={{ flex: 1 }}>
                <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>Cosmos found at {target.h}</Text>
                <Text style={[type.footnote, { color: t.textSecondary }]}>
                  {probe.info?.version ? `Cosmos ${probe.info.version}` : 'Engine reachable'}{probe.info?.deviceId ? ` · ${probe.info.deviceId.slice(0, 8)}` : ''}
                </Text>
              </View>
            </View>
          ) : probe.state === 'fail' ? (
            <View style={[styles.result, { backgroundColor: alpha(t.danger, 0.1) }]} accessibilityLiveRegion="polite">
              <Icon name="wifi.exclamationmark" size={20} color={t.danger} />
              <View style={{ flex: 1 }}>
                <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>Nothing answered at {target.h}:{target.p}</Text>
                <Text style={[type.footnote, { color: t.textSecondary }]}>Check that Cosmos is open on the Mac and both devices share a network.</Text>
              </View>
            </View>
          ) : null}

          <View style={styles.actions}>
            {found ? (
              <Button title="Trust & Link" icon="link" onPress={() => void link()} loading={busy} />
            ) : (
              <Button title="Check Connection" variant={host.trim() ? 'primary' : 'secondary'} onPress={() => void probeHost()} disabled={!host.trim()} loading={probe.state === 'probing'} />
            )}
            <Text style={[type.caption, { color: t.textTertiary, textAlign: 'center' }]}>
              Pairing speaks the engine protocol directly over your network. No cloud, no relay.
            </Text>
          </View>

          <SectionLabel>Remote (Woozlit)</SectionLabel>
          {auth ? (
            remoteCandidates.length === 0 ? (
              <Group footer="Desktops signed into your account appear here when they're reachable.">
                <Row title="No other devices online" leading={<IconTile name="icloud.slash" tint={t.textTertiary} />} />
              </Group>
            ) : (
              <Group>
                {remoteCandidates.map((d) => (
                  <Row
                    key={d.id}
                    tall
                    title={d.name}
                    subtitle={`${d.platform}${d.version ? ` · Cosmos ${d.version}` : ''}`}
                    leading={<IconTile name="desktopcomputer" tint={t.link} size={32} />}
                    trailing={busy ? <ActivityIndicator size="small" /> : <Text style={[type.body, { color: t.link }]}>Link</Text>}
                    onPress={() => void linkRemote(d.id, d.name)}
                    disabled={busy}
                  />
                ))}
              </Group>
            )
          ) : (
            <Group footer="Remote reach is roadmap; signing in shows which of your desktops are online.">
              <Row title="Sign in to Woozlit" subtitle="See your desktops anywhere" icon="person.crop.circle" iconTint={t.success} onPress={() => router.push('/sign-in')} chevron />
            </Group>
          )}
        </ScrollView>
      </KeyboardAvoidingView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: spacing(4), paddingTop: spacing(2), paddingBottom: spacing(10) },
  steps: { gap: spacing(3), paddingHorizontal: spacing(1), paddingBottom: spacing(2) },
  step: { flexDirection: 'row', alignItems: 'flex-start', gap: spacing(3) },
  stepBadge: { width: 24, height: 24, borderRadius: 12, alignItems: 'center', justifyContent: 'center', marginTop: 1 },
  sep: { height: StyleSheet.hairlineWidth, marginLeft: spacing(4) },
  result: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    padding: spacing(3.5), marginTop: spacing(3), borderRadius: radii.lg, borderCurve: 'continuous',
  },
  actions: { marginTop: spacing(4), gap: spacing(3) },
});
