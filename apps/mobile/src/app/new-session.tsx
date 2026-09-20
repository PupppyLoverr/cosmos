import React, { useEffect, useMemo, useState } from 'react';
import { ActionSheetIOS, Alert, KeyboardAvoidingView, Modal, Platform, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useCosmos, useSnapshot } from '../lib/client';
import { HarnessId, ReasoningLevel, SandboxLevel } from '../lib/protocol';
import { PickedFile, ensureDecodable, fileToBase64Chunks } from '../lib/attachments';
import { radii, spacing, type, useTheme } from '../lib/theme';
import { Row, Group, SectionLabel, Segmented, Notice, Icon, Caption } from '../components/bits';
import { HarnessMark, harnessLabel } from '../components/harness-mark';
import { Composer } from '../components/composer';
import { Glass, Screen } from '../components/surface';

const REASONING: { id: ReasoningLevel; label: string }[] = [
  { id: 'low', label: 'Low' },
  { id: 'medium', label: 'Medium' },
  { id: 'high', label: 'High' },
];

const SANDBOX: { id: SandboxLevel; label: string; icon: string; hint: string }[] = [
  { id: 'read-only', label: 'Read only', icon: 'eye', hint: 'Can inspect files but not change them.' },
  { id: 'workspace-write', label: 'Workspace', icon: 'folder', hint: 'Edits stay inside the project folder.' },
  { id: 'danger-full-access', label: 'Full access', icon: 'exclamationmark.shield', hint: 'Unrestricted — including Computer Use on the Mac.' },
];

const PROMPTS = [
  'Summarise what changed in this repo today',
  'Open Finder and tidy my Downloads folder',
  'Find and fix the failing test',
];

function pickFrom(title: string, options: string[], current: string | undefined, onPick: (index: number) => void) {
  if (Platform.OS === 'ios') {
    ActionSheetIOS.showActionSheetWithOptions(
      { title, options: [...options.map((o) => (o === current ? `✓ ${o}` : o)), 'Cancel'], cancelButtonIndex: options.length },
      (i) => { if (i < options.length) onPick(i); },
    );
  } else {
    Alert.alert(title, undefined, [
      ...options.map((o, i) => ({ text: o === current ? `✓ ${o}` : o, onPress: () => onPick(i) })),
      { text: 'Cancel', style: 'cancel' as const },
    ]);
  }
}

export default function NewSessionScreen() {
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const seed = useLocalSearchParams<{ prompt?: string }>().prompt;
  const client = useCosmos();
  const spaces = useSnapshot(client.spaces);
  const connection = useSnapshot(client.connection);
  const [harnesses, setHarnesses] = useState<{ id: HarnessId; name?: string }[]>([]);
  const [modelsFor, setModelsFor] = useState<{ harness: HarnessId; list: { id: string; name?: string }[] }>({ harness: 'opencode', list: [] });
  const [harness, setHarness] = useState<HarnessId>('opencode');
  const [model, setModel] = useState<string | null>(null);
  const [reasoning, setReasoning] = useState<ReasoningLevel>('medium');
  const [sandbox, setSandbox] = useState<SandboxLevel>('workspace-write');
  const [spaceId, setSpaceId] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<PickedFile[]>([]);
  const [busy, setBusy] = useState(false);
  const [setup, setSetup] = useState(false);
  const [seedText, setSeedText] = useState(seed ?? '');
  const [typed, setTyped] = useState(false);

  const online = connection.status === 'connected';
  const desktopName = connection.desktop?.name;

  useEffect(() => {
    if (!online) return;
    void client.listHarnesses().then((list) => {
      setHarnesses(list);
      if (list.length && !list.some((h) => h.id === harness)) setHarness(list[0]!.id);
    }).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, online]);

  useEffect(() => {
    if (!online || !harness) return;
    let cancelled = false;
    void client.listModels(harness)
      .then((list) => { if (!cancelled) setModelsFor({ harness, list }); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [client, online, harness]);
  const models = modelsFor.harness === harness ? modelsFor.list : [];
  const modelName = model ? (models.find((m) => m.id === model)?.name ?? model) : 'Default';

  const spaceChoices = useMemo(
    () => [{ id: null as string | null, label: 'Desktop', path: undefined as string | undefined }, ...spaces.map((s) => ({ id: s.id as string | null, label: s.name || s.path.split('/').pop() || s.path, path: s.path }))],
    [spaces],
  );
  const space = spaceChoices.find((s) => s.id === spaceId) ?? spaceChoices[0]!;
  const harnessChoices = harnesses.length ? harnesses.filter((h) => h.id !== 'mock') : [{ id: harness }];
  const harnessName = harnesses.find((h) => h.id === harness)?.name ?? harnessLabel(harness) ?? String(harness);

  // Start = create the chat, upload anything attached, dispatch the first turn,
  // and land in the transcript. No intermediate confirmation screen.
  const start = async (text: string) => {
    if (!online) return;
    setBusy(true);
    try {
      const s = spaces.find((x) => x.id === spaceId);
      const paths: string[] = [];
      for (const f of attachments) {
        const n = await ensureDecodable(f);
        paths.push(await client.uploadAttachment(n.name, await fileToBase64Chunks(n)));
      }
      const prompt = paths.length
        ? `${text}\n\nAttached files (uploaded):\n${paths.map((p) => `- ${p}`).join('\n')}`
        : text;
      const chatId = await client.createChat(spaceId, s?.deviceId ?? null, { harness, model, reasoning, sandbox, modelOptions: {} });
      if (prompt.trim()) {
        await client.run(chatId, { prompt: prompt.trim(), harness, model, reasoning, cwd: s?.path ?? '~', sandbox, modelOptions: {}, attachments: paths });
      }
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      // Deep-link cold opens land here with no screen below, so back() would
      // warn GO_BACK-unhandled; replace puts the session in this slot either way.
      router.replace({ pathname: '/session/[id]', params: { id: chatId, title: text.trim().slice(0, 40) || 'Session' } });
    } catch (e) {
      Alert.alert("Couldn't start the session", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const summary = [harnessName, modelName !== 'Default' ? modelName : null, REASONING.find((r) => r.id === reasoning)!.label, space.label]
    .filter(Boolean)
    .join(' · ');

  return (
    <Screen>
      <KeyboardAvoidingView style={styles.flex} behavior={Platform.OS === 'ios' ? 'padding' : undefined} keyboardVerticalOffset={insets.top + 44}>
        <View style={styles.flex}>
          {!online ? (
            <View style={styles.notice}><Notice kind="warning" title="Desktop offline" body="Link or reconnect a desktop before starting a session." /></View>
          ) : null}

          <View style={styles.canvas}>
            <HarnessMark harness={harness} size={44} />
            <Text style={[type.title3, styles.canvasTitle, { color: t.text }]}>{desktopName ? `New session on ${desktopName}` : 'New session'}</Text>
            <Caption style={styles.canvasBody}>{`${harnessName} does the reasoning. Cosmos runs it on the Mac.`}</Caption>
          </View>

          {!seedText && !typed ? (
            <View style={styles.ideas}>
              {PROMPTS.map((p) => (
                <Pressable
                  key={p}
                  onPress={() => { void Haptics.selectionAsync(); setSeedText(p); }}
                  style={({ pressed }) => [styles.idea, { opacity: pressed ? 0.6 : 1 }]}
                  accessibilityRole="button"
                >
                  <Icon name="arrow.up.left" size={12} color={t.textTertiary} />
                  <Caption style={styles.ideaText}>{p}</Caption>
                </Pressable>
              ))}
            </View>
          ) : null}
        </View>

        <Pressable
          onPress={() => { void Haptics.selectionAsync(); setSetup(true); }}
          style={({ pressed }) => [styles.setupChip, { opacity: pressed ? 0.7 : 1 }]}
          accessibilityRole="button"
          accessibilityLabel={`Session setup: ${summary}`}
          accessibilityHint="Agent, model, effort and project"
        >
          <Glass radius={radii.pill} thin style={[styles.setupInner, { borderColor: t.glassStroke }]}>
            <HarnessMark harness={harness} size={16} />
            <Text style={[type.footnote, { color: t.textSecondary, fontWeight: '500' }]} numberOfLines={1}>{summary}</Text>
            <Icon name="chevron.up" size={10} color={t.textTertiary} weight="semibold" />
          </Glass>
        </Pressable>

        <Composer
          key={seedText}
          placeholder="Message Cosmos…"
          initialText={seedText}
          onChangeText={(v) => setTyped(v.length > 0)}
          running={false}
          sending={busy}
          attachments={attachments}
          onAddAttachment={(f) => setAttachments((a) => [...a, f])}
          onRemoveAttachment={(i) => setAttachments((a) => a.filter((_, j) => j !== i))}
          onSend={(text) => void start(text)}
          onStop={() => {}}
          bottomInset={Math.max(insets.bottom, spacing(2))}
        />
      </KeyboardAvoidingView>

      <Modal visible={setup} animationType="slide" presentationStyle="pageSheet" onRequestClose={() => setSetup(false)}>
        <View style={[styles.flex, { backgroundColor: t.background }]}>
          <View style={styles.sheetHeader}>
            <Text style={[type.headline, { color: t.text }]}>Session setup</Text>
            <Pressable onPress={() => setSetup(false)} hitSlop={10} accessibilityRole="button" accessibilityLabel="Done">
              <Text style={[type.body, { color: t.link, fontWeight: '600' }]}>Done</Text>
            </Pressable>
          </View>
          <ScrollView contentContainerStyle={styles.sheetContent} keyboardShouldPersistTaps="handled">
            <Group header="Agent">
              {harnessChoices.map((h) => (
                <Row
                  key={h.id}
                  title={h.name ?? harnessLabel(h.id) ?? String(h.id)}
                  leading={<HarnessMark harness={h.id} size={26} />}
                  selected={h.id === harness}
                  onPress={() => { void Haptics.selectionAsync(); setHarness(h.id); setModel(null); }}
                />
              ))}
            </Group>

            <Group style={styles.sheetGroup}>
              <Row
                title="Model"
                value={modelName}
                chevron
                onPress={() => pickFrom('Model', ['Default', ...models.map((m) => m.name ?? m.id)], modelName, (i) => setModel(i === 0 ? null : models[i - 1]!.id))}
              />
              <Row
                title="Project"
                value={space.label}
                chevron
                onPress={() => pickFrom('Project', spaceChoices.map((s) => s.label), space.label, (i) => setSpaceId(spaceChoices[i]!.id))}
              />
            </Group>
            {space.path ? <Caption style={styles.sheetFooter}>{space.path}</Caption> : null}

            <SectionLabel>Effort</SectionLabel>
            <Segmented options={REASONING} value={reasoning} onChange={setReasoning} />

            <SectionLabel>Permissions</SectionLabel>
            <Segmented options={SANDBOX.map(({ id, label, icon }) => ({ id, label, icon }))} value={sandbox} onChange={setSandbox} />
            <Text style={[type.footnote, styles.sheetFooter, { color: sandbox === 'danger-full-access' ? t.warning : t.textSecondary }]}>
              {SANDBOX.find((s) => s.id === sandbox)!.hint}
            </Text>
          </ScrollView>
        </View>
      </Modal>
    </Screen>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  notice: { paddingHorizontal: spacing(4), paddingTop: spacing(3) },
  canvas: { flex: 1, alignItems: 'center', justifyContent: 'center', paddingHorizontal: spacing(8), gap: spacing(2) },
  canvasTitle: { textAlign: 'center', marginTop: spacing(2) },
  canvasBody: { textAlign: 'center' },
  ideas: { paddingHorizontal: spacing(5), paddingBottom: spacing(3), gap: spacing(1) },
  idea: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), minHeight: 36 },
  ideaText: { flex: 1 },
  setupChip: { alignSelf: 'flex-start', marginLeft: spacing(4), marginBottom: spacing(1) },
  setupInner: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), paddingLeft: spacing(2), paddingRight: spacing(2.5), height: 30, borderWidth: StyleSheet.hairlineWidth, maxWidth: 320 },
  sheetHeader: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: spacing(4), paddingTop: spacing(4), paddingBottom: spacing(2) },
  sheetContent: { paddingHorizontal: spacing(4), paddingBottom: spacing(10) },
  sheetGroup: { marginTop: spacing(4) },
  sheetFooter: { marginTop: spacing(2), marginHorizontal: spacing(2) },
});
