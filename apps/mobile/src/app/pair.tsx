import React, { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, KeyboardAvoidingView, Platform, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { CameraView, useCameraPermissions } from 'expo-camera';
import { AccountDevice, PLACEHOLDER_NAME, useCosmos, useSnapshot } from '../lib/client';
import { alpha, radii, spacing, type, useTheme } from '../lib/theme';
import { Button, Field, Group, Icon, IconTile, Row, SectionLabel } from '../components/bits';
import { localSpec, RpcClient } from '../lib/rpc';
import { EngineInfo } from '../lib/protocol';
import { Screen } from '../components/surface';

type Probe = { state: 'idle' | 'probing' | 'ok' | 'fail'; info?: EngineInfo; error?: string };

const DEFAULT_PORT = 27654;
const CODE_RE = /^[A-Z2-9]{6}$/;

// Accepts `host` or `host:port`; the engine's IPC port only differs for side-by-side/dev instances.
function hostParts(raw: string): { h: string; p: number } {
  const s = raw.trim();
  const m = /^(\S+?):(\d{1,5})$/.exec(s);
  if (m) return { h: m[1], p: Math.min(65535, parseInt(m[2], 10)) };
  return { h: s, p: DEFAULT_PORT };
}

/** `cosmos://pair?d=<deviceId>&c=<code>` from the desktop QR — scheme and
 *  path are required so an unrelated QR never initiates pairing. */
function parseQrPayload(data: string): { deviceId: string; code: string } | null {
  const m = /^cosmos:\/\/pair\?([?&\S]*)$/i.exec(data.trim());
  if (!m) return null;
  const q = m[1];
  const d = /(?:^|&)d=([^&\s]+)/.exec(q)?.[1];
  const c = /(?:^|&)c=([A-Za-z2-9]{6})(?:&|$)/.exec(q)?.[1];
  if (!d || !c) return null;
  return { deviceId: decodeURIComponent(d), code: c.toUpperCase() };
}

/** The desktop groups the code for display (`ABC 123`); typed input may
 *  carry that separator — strip spaces/dashes before validating. */
const normalizeCode = (v: string) => v.replace(/[\s-]/g, '').toUpperCase();

export default function PairScreen() {
  const t = useTheme();
  const client = useCosmos();
  const auth = useSnapshot(client.auth);
  const desktops = useSnapshot(client.desktops);

  const [code, setCode] = useState('');
  const [host, setHost] = useState('');
  const [name, setName] = useState('');
  const [probe, setProbe] = useState<Probe>({ state: 'idle' });
  const [busy, setBusy] = useState(false);
  const [pairError, setPairError] = useState<string | null>(null);
  const [accountDevices, setAccountDevices] = useState<AccountDevice[] | null>(null);
  const [permission, requestPermission] = useCameraPermissions();
  const scanning = useRef(true);

  useEffect(() => {
    if (!auth) return;
    void client.fetchAccountDevices().then(setAccountDevices);
  }, [auth, client]);

  useEffect(() => {
    if (auth && permission && !permission.granted && permission.canAskAgain) {
      void requestPermission();
    }
  }, [auth, permission, requestPermission]);

  const leave = () => { if (router.canGoBack()) router.back(); else router.replace('/'); };

  const pairAndConnect = async (deviceId: string, codeValue: string) => {
    setBusy(true);
    setPairError(null);
    try {
      const desktop = await client.pairRemote(deviceId, codeValue, 'iPhone');
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      await client.connect(desktop);
      leave();
    } catch (e) {
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error);
      setPairError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
      scanning.current = true;
    }
  };

  const onBarcode = ({ data }: { data: string }) => {
    if (!scanning.current || busy) return;
    const payload = parseQrPayload(data);
    if (!payload) return;
    scanning.current = false;
    void pairAndConnect(payload.deviceId, payload.code);
  };

  const submitCode = async () => {
    const c = normalizeCode(code);
    if (!CODE_RE.test(c)) return;
    setBusy(true);
    setPairError(null);
    try {
      const resolved = await client.resolvePairCode(c);
      if (!resolved) {
        setPairError("That code isn't live — ask the desktop for a fresh one (they last 5 minutes).");
        return;
      }
      await pairAndConnect(resolved.deviceId, c);
    } catch (e) {
      setPairError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

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

  const found = probe.state === 'ok';
  const codeValid = CODE_RE.test(normalizeCode(code));
  // A remote record only counts as paired when it carries a grant token —
  // legacy pre-pairing entries can still dial but will be gated, so they
  // must show "Needs code" instead of "Paired".
  const knownRemoteIds = new Set(
    desktops.filter((d) => d.kind === 'remote' && d.deviceToken).map((d) => d.address),
  );

  return (
    <Screen>
      <KeyboardAvoidingView style={{ flex: 1 }} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
        <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content} keyboardDismissMode="interactive" keyboardShouldPersistTaps="handled">
          {!auth ? (
            <Group footer="Remote pairing needs the same account on both devices. Same-Wi-Fi linking is below if you skip this.">
              <Row title="Sign in to Woozlit" subtitle="Required to pair over any network" icon="person.crop.circle" onPress={() => router.push('/sign-in')} chevron />
            </Group>
          ) : (
            <>
              <SectionLabel>Pair phone</SectionLabel>
              {permission?.granted ? (
                <View style={[styles.cameraCard, { borderColor: t.separator }]}>
                  <CameraView
                    style={styles.camera}
                    facing="back"
                    barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
                    onBarcodeScanned={onBarcode}
                  />
                  <Text style={[type.caption, { color: t.textSecondary, textAlign: 'center', paddingVertical: spacing(2) }]}>
                    Point at the QR in Cosmos → Settings → Devices → Pair phone
                  </Text>
                </View>
              ) : (
                <Group footer={permission?.canAskAgain === false ? 'Camera permission is off — type the code instead.' : undefined}>
                  <Row
                    title="Scan the desktop QR"
                    subtitle="Cosmos → Settings → Devices → Pair phone"
                    leading={<IconTile name="camera" tint={t.link} />}
                    trailing={<Text style={[type.body, { color: t.link }]}>Allow</Text>}
                    onPress={() => void requestPermission()}
                  />
                </Group>
              )}

              <Group footer="Single use, good for 5 minutes. A fresh code is on the desktop screen if this one lapsed.">
                <Field
                  label="Code"
                  placeholder="ABC 123"
                  value={code}
                  onChangeText={(v) => { setCode(v.toUpperCase()); setPairError(null); }}
                  autoCapitalize="characters"
                  autoCorrect={false}
                  maxLength={7}
                  returnKeyType="go"
                  onSubmitEditing={() => void submitCode()}
                />
              </Group>
              {pairError ? (
                <View style={[styles.result, { backgroundColor: alpha(t.danger, 0.1) }]} accessibilityLiveRegion="polite">
                  <Icon name="exclamationmark.triangle" size={20} color={t.danger} />
                  <Text style={[type.footnote, { color: t.textSecondary, flex: 1 }]}>{pairError}</Text>
                </View>
              ) : null}
              <View style={styles.actions}>
                <Button title="Pair" onPress={() => void submitCode()} disabled={!codeValid} loading={busy} />
              </View>

              <SectionLabel>On your account</SectionLabel>
              <Group footer="Already-paired desktops connect here. Others need the code shown on their own screen.">
                {accountDevices === null ? (
                  <Row title="Looking for your desktops…" leading={<IconTile name="magnifyingglass" tint={t.textTertiary} />} />
                ) : accountDevices.length === 0 ? (
                  <Row title="No desktops on this account" leading={<IconTile name="icloud.slash" tint={t.textTertiary} />} />
                ) : (
                  accountDevices.map((d) => {
                    const paired = knownRemoteIds.has(d.deviceId);
                    const linked = desktops.find((x) => x.kind === 'remote' && x.address === d.deviceId);
                    return (
                      <Row
                        key={d.deviceId}
                        tall
                        title={d.name}
                        subtitle={`${d.platform ?? 'desktop'} · ${d.online ? 'online' : 'offline'}${paired ? ' · paired' : ''}`}
                        leading={<IconTile name="desktopcomputer" tint={d.online ? t.link : t.textTertiary} size={32} />}
                        trailing={
                          paired && linked ? (
                            busy ? <ActivityIndicator size="small" /> : <Text style={[type.body, { color: t.link }]}>Connect</Text>
                          ) : (
                            <Text style={[type.footnote, { color: t.textTertiary }]}>Needs code</Text>
                          )
                        }
                        onPress={paired && linked ? () => { setBusy(true); void client.connect(linked).then(leave).finally(() => setBusy(false)); } : undefined}
                        disabled={busy || !paired}
                      />
                    );
                  })
                )}
              </Group>
            </>
          )}

          <SectionLabel>Same network (advanced)</SectionLabel>
          <Group footer={found ? undefined : `Direct engine link on port ${DEFAULT_PORT} — both devices on one Wi-Fi.`}>
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
            />
            <View style={[styles.sep, { backgroundColor: t.separator }]} />
            <Field label="Name" placeholder="Uses the computer's name" value={name} onChangeText={setName} returnKeyType="done" />
          </Group>

          {probe.state === 'ok' ? (
            <View style={[styles.result, { backgroundColor: alpha(t.success, 0.12) }]} accessibilityLiveRegion="polite">
              <Icon name="checkmark.seal.fill" size={20} color={t.success} />
              <View style={{ flex: 1 }}>
                <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>Cosmos found at {hostParts(host).h}</Text>
                <Text style={[type.footnote, { color: t.textSecondary }]}>
                  {probe.info?.version ? `Cosmos ${probe.info.version}` : 'Engine reachable'}{probe.info?.deviceId ? ` · ${probe.info.deviceId.slice(0, 8)}` : ''}
                </Text>
              </View>
            </View>
          ) : probe.state === 'fail' ? (
            <View style={[styles.result, { backgroundColor: alpha(t.danger, 0.1) }]} accessibilityLiveRegion="polite">
              <Icon name="wifi.exclamationmark" size={20} color={t.danger} />
              <View style={{ flex: 1 }}>
                <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>Nothing answered at {hostParts(host).h}:{hostParts(host).p}</Text>
                <Text style={[type.footnote, { color: t.textSecondary }]}>Check that Cosmos is open on the desktop and both devices share a network.</Text>
              </View>
            </View>
          ) : null}

          {host.trim() ? (
            <View style={styles.actions}>
              {found ? (
                <Button title="Trust & Link" icon="link" onPress={() => void link()} loading={busy} />
              ) : (
                <Button title="Check Connection" variant="secondary" onPress={() => void probeHost()} loading={probe.state === 'probing'} />
              )}
            </View>
          ) : null}
        </ScrollView>
      </KeyboardAvoidingView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: spacing(4), paddingTop: spacing(2), paddingBottom: spacing(10) },
  sep: { height: StyleSheet.hairlineWidth, marginLeft: spacing(4) },
  result: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    padding: spacing(3.5), marginTop: spacing(3), borderRadius: radii.lg, borderCurve: 'continuous',
  },
  actions: { marginTop: spacing(4), gap: spacing(3) },
  cameraCard: {
    borderRadius: radii.lg, borderCurve: 'continuous', overflow: 'hidden',
    borderWidth: StyleSheet.hairlineWidth,
  },
  camera: { height: 240, width: '100%' },
});
