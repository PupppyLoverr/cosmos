import React, { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, KeyboardAvoidingView, LayoutAnimation, Platform, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { CameraView, useCameraPermissions } from 'expo-camera';
import { AccountDevice, PLACEHOLDER_NAME, useCosmos, useSnapshot } from '../lib/client';
import { alpha, motion, radii, spacing, type, useTheme } from '../lib/theme';
import { Button, Field, Group, Icon, IconButton, IconTile, Notice, Row, SectionLabel } from '../components/bits';
import { localSpec, RpcClient } from '../lib/rpc';
import { EngineInfo } from '../lib/protocol';
import { useReduceMotion } from '../lib/motion';

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

function AdvancedNetworkSection({
  host,
  onHost,
  name,
  setName,
  probe,
  probeHost,
  found,
  link,
  busy,
  initialOpen,
}: {
  initialOpen?: boolean;
  host: string;
  onHost: (value: string) => void;
  name: string;
  setName: (value: string) => void;
  probe: Probe;
  probeHost: () => void;
  found: boolean;
  link: () => void;
  busy: boolean;
}) {
  const t = useTheme();
  const reduce = useReduceMotion();
  const [advanced, setAdvanced] = useState(!!host || !!initialOpen);
  const toggle = () => {
    if (!reduce) {
      LayoutAnimation.configureNext({
        duration: motion.collapse,
        update: { type: LayoutAnimation.Types.easeInEaseOut },
        create: { type: LayoutAnimation.Types.easeInEaseOut, property: LayoutAnimation.Properties.opacity },
        delete: { type: LayoutAnimation.Types.easeInEaseOut, property: LayoutAnimation.Properties.opacity },
      });
    }
    setAdvanced((value) => !value);
  };

  return (
    <Group footer={found ? undefined : `Direct engine link on port ${DEFAULT_PORT} — both devices on one Wi-Fi.`}>
      <Row
        title="On this network"
        subtitle="Link directly over one Wi-Fi"
        icon="wifi"
        onPress={toggle}
        trailing={<Icon name={advanced ? 'chevron.down' : 'chevron.right'} size={13} color={t.textTertiary} weight="semibold" />}
        accessibilityLabel={`On this network, ${advanced ? 'expanded' : 'collapsed'}`}
      />
      {advanced ? (
        <View style={styles.advancedContent}>
          <Field
            label="Address"
            placeholder="192.168.1.20"
            value={host}
            onChangeText={onHost}
            autoCapitalize="none"
            autoCorrect={false}
            keyboardType="numbers-and-punctuation"
            returnKeyType="go"
            onSubmitEditing={probeHost}
          />
          <View style={[styles.sep, { backgroundColor: t.separator }]} />
          <Field label="Name" placeholder="Uses the computer's name" value={name} onChangeText={setName} returnKeyType="done" />

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
            <Notice
              kind="error"
              title={`Nothing answered at ${hostParts(host).h}:${hostParts(host).p}`}
              body="Check that Cosmos is open on the desktop and both devices share a network."
            />
          ) : null}

          {host.trim() ? (
            <View style={styles.actions}>
              {found ? (
                <Button title="Trust & Link" icon="link" onPress={link} loading={busy} />
              ) : (
                <Button title="Check Connection" variant="secondary" onPress={probeHost} loading={probe.state === 'probing'} />
              )}
            </View>
          ) : null}
        </View>
      ) : null}
    </Group>
  );
}

export default function PairScreen() {
  const t = useTheme();
  const { mode } = useLocalSearchParams<{ mode?: string }>();
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
    <View style={[styles.sheet, { backgroundColor: t.background }]}>
      <KeyboardAvoidingView style={{ flex: 1 }} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
        <ScrollView
          contentInsetAdjustmentBehavior="automatic"
          contentContainerStyle={styles.content}
          keyboardDismissMode="interactive"
          keyboardShouldPersistTaps="handled"
          automaticallyAdjustKeyboardInsets
        >
          <View style={styles.headerRow}>
            <View style={styles.headerCopy}>
              <Text accessibilityRole="header" style={[type.title2, { color: t.text }]}>Pair this phone</Text>
              <Text style={[type.footnote, { color: t.textSecondary }]}>Scan the code in Cosmos on your desktop. Works from any network.</Text>
            </View>
            <IconButton icon="xmark" label="Close" size={32} tint={t.textSecondary} onPress={leave} />
          </View>
          {!auth ? (
            <View style={styles.signIn}>
              <Button title="Continue with Woozlit" onPress={() => router.push('/sign-in')} />
              <Text style={[type.footnote, { color: t.textSecondary, textAlign: 'center' }]}>
                Pairing over any network needs the same account on both devices.
              </Text>
            </View>
          ) : (
            <>
              {permission?.granted ? (
                <View style={[styles.cameraCard, { borderColor: t.separator }]}>
                  <CameraView
                    style={styles.camera}
                    facing="back"
                    barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
                    onBarcodeScanned={onBarcode}
                  />
                  <View pointerEvents="none" style={styles.scanOverlay}>
                    <View style={[styles.corner, styles.cornerTopLeft]} />
                    <View style={[styles.corner, styles.cornerTopRight]} />
                    <View style={[styles.corner, styles.cornerBottomLeft]} />
                    <View style={[styles.corner, styles.cornerBottomRight]} />
                    <View style={styles.scanCaption}>
                      <Text style={[type.footnote, { color: '#FFFFFF', fontWeight: '500' }]}>Point at the code on the desktop</Text>
                    </View>
                  </View>
                </View>
              ) : (
                <Group footer={permission?.canAskAgain === false ? 'Camera permission is off — type the code instead.' : undefined}>
                  <Row
                    title="Scan the desktop QR"
                    subtitle="Cosmos → Settings → Devices → Pair phone"
                    leading={<IconTile name="camera" />}
                    trailing={<Text style={[type.body, { color: t.link }]}>Allow</Text>}
                    onPress={() => void requestPermission()}
                  />
                </Group>
              )}

              <View style={styles.dividerRow}>
                <View style={[styles.divider, { backgroundColor: t.separator }]} />
                <Text style={[type.caption, { color: t.textSecondary }]}>or type the code</Text>
                <View style={[styles.divider, { backgroundColor: t.separator }]} />
              </View>
              <TextInput
                style={[styles.codeInput, type.title2, { color: t.text, backgroundColor: t.surface, borderColor: t.separator }]}
                value={code}
                onChangeText={(v) => { setCode(v.toUpperCase()); setPairError(null); }}
                autoCapitalize="characters"
                autoCorrect={false}
                maxLength={7}
                returnKeyType="go"
                onSubmitEditing={() => void submitCode()}
                placeholder="ABC123"
                placeholderTextColor={t.textTertiary}
                accessibilityLabel="Pairing code"
                keyboardAppearance={t.scheme}
              />
              <Text style={[type.caption, styles.codeHint, { color: t.textSecondary }]}>Single use, good for 5 minutes. A fresh code is on the desktop screen if this one lapsed.</Text>
              {pairError ? (
                <Notice kind="error" title="Couldn't pair" body={pairError} />
              ) : null}
              <View style={styles.actions}>
                <Button title="Pair" style={styles.fullButton} onPress={() => void submitCode()} disabled={!codeValid} loading={busy} />
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
                        leading={<IconTile name="desktopcomputer" size={32} />}
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

          <AdvancedNetworkSection
            host={host}
            onHost={onHost}
            name={name}
            setName={setName}
            probe={probe}
            probeHost={() => void probeHost()}
            found={found}
            link={() => void link()}
            busy={busy}
            initialOpen={mode === 'local'}
          />
        </ScrollView>
      </KeyboardAvoidingView>
    </View>
  );
}

const styles = StyleSheet.create({
  sheet: { flex: 1 },
  content: { paddingHorizontal: spacing(4), paddingTop: spacing(6), paddingBottom: spacing(10), gap: spacing(3) },
  headerRow: { flexDirection: 'row', alignItems: 'flex-start', justifyContent: 'space-between', gap: spacing(3), marginBottom: spacing(2) },
  signIn: { gap: spacing(3), marginTop: spacing(2) },
  headerCopy: { flex: 1, gap: spacing(1) },
  sep: { height: StyleSheet.hairlineWidth, marginLeft: spacing(4) },
  result: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    padding: spacing(3.5), marginTop: spacing(3), borderRadius: radii.lg, borderCurve: 'continuous',
  },
  actions: { marginTop: spacing(4), gap: spacing(3) },
  cameraCard: {
    width: '100%', aspectRatio: 1, borderRadius: radii.xl, borderCurve: 'continuous', overflow: 'hidden',
    borderWidth: StyleSheet.hairlineWidth, position: 'relative',
  },
  camera: StyleSheet.absoluteFill,
  scanOverlay: StyleSheet.absoluteFill,
  corner: { position: 'absolute', width: 28, height: 28, borderWidth: 2, borderColor: 'rgba(255,255,255,0.9)' },
  cornerTopLeft: { top: spacing(6), left: spacing(6), borderRightWidth: 0, borderBottomWidth: 0 },
  cornerTopRight: { top: spacing(6), right: spacing(6), borderLeftWidth: 0, borderBottomWidth: 0 },
  cornerBottomLeft: { bottom: spacing(6), left: spacing(6), borderRightWidth: 0, borderTopWidth: 0 },
  cornerBottomRight: { bottom: spacing(6), right: spacing(6), borderLeftWidth: 0, borderTopWidth: 0 },
  scanCaption: {
    position: 'absolute', bottom: spacing(4), alignSelf: 'center', paddingHorizontal: spacing(3), paddingVertical: spacing(1.5),
    borderRadius: radii.chip, backgroundColor: 'rgba(0,0,0,0.55)',
  },
  dividerRow: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), marginTop: spacing(2) },
  divider: { flex: 1, height: StyleSheet.hairlineWidth },
  codeInput: {
    width: '100%', height: 56, borderRadius: radii.md, borderCurve: 'continuous',
    borderWidth: StyleSheet.hairlineWidth, textAlign: 'center', fontFamily: type.mono.fontFamily,
    letterSpacing: 6, paddingHorizontal: spacing(3), marginTop: spacing(1),
  },
  codeHint: { textAlign: 'center', marginTop: spacing(1) },
  fullButton: { width: '100%' },
  advancedContent: { paddingBottom: spacing(3) },
});
