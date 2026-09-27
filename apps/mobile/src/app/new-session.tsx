// New session (Codex "New thread" + Cursor composer): a quiet centred
// question with the workspace picker under it, and the composer card docked
// at the bottom carrying Access · Harness · Model as tool pills.

import React, { useEffect, useMemo, useState } from 'react';
import { Alert, KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { desktopDisplayName, useCosmos, useSnapshot } from '../lib/client';
import { HarnessId, ReasoningLevel, SandboxLevel } from '../lib/protocol';
import { PickedFile, ensureDecodable, fileToBase64Chunks } from '../lib/attachments';
import { showMenu } from '../lib/menu';
import { spacing, type, useTheme } from '../lib/theme';
import { Icon, Notice } from '../components/bits';
import { CircleButton, TopBar, useDockInset } from '../components/chrome';
import { HarnessMark, harnessLabel } from '../components/harness-mark';
import { Composer, ComposerChip } from '../components/composer';
import { Screen } from '../components/surface';

// Effort stays at the engine default; it is adjustable per session from the thread.
const REASONING: ReasoningLevel = 'medium';

const ACCESS: { id: SandboxLevel; label: string; hint: string }[] = [
  { id: 'read-only', label: 'Read only', hint: 'Can inspect files but not change them' },
  { id: 'workspace-write', label: 'Workspace', hint: 'Edits stay inside the project folder' },
  { id: 'danger-full-access', label: 'Full access', hint: 'Unrestricted, including Computer Use' },
];

// Prompts that map to things Cosmos actually does on the desktop.
const PROMPTS: { icon: string; label: string }[] = [
  { icon: 'folder', label: 'Open the project on the desktop' },
  { icon: 'display', label: 'Check what is on the screen right now' },
];

export default function NewSessionScreen() {
  const t = useTheme();
  const seed = useLocalSearchParams<{ prompt?: string }>().prompt;
  const client = useCosmos();
  const spaces = useSnapshot(client.spaces);
  const connection = useSnapshot(client.connection);
  const [harnesses, setHarnesses] = useState<{ id: HarnessId; name?: string }[]>([]);
  const [modelsFor, setModelsFor] = useState<{ harness: HarnessId; list: { id: string; name?: string }[] }>({ harness: 'opencode', list: [] });
  const [harness, setHarness] = useState<HarnessId>('opencode');
  const [model, setModel] = useState<string | null>(null);
  const [sandbox, setSandbox] = useState<SandboxLevel>('workspace-write');
  const [spaceId, setSpaceId] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<PickedFile[]>([]);
  const [draft, setDraft] = useState(seed ?? '');
  const [typed, setTyped] = useState(!!seed);
  const [busy, setBusy] = useState(false);
  const dockInset = useDockInset();

  const online = connection.status === 'connected';

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
    () => [
      { id: null as string | null, label: 'Current checkout' },
      ...spaces.map((s) => ({ id: s.id as string | null, label: s.name || s.path.split('/').pop() || s.path })),
    ],
    [spaces],
  );
  const space = spaceChoices.find((s) => s.id === spaceId) ?? spaceChoices[0]!;
  const harnessChoices = harnesses.length ? harnesses.filter((h) => h.id !== 'mock') : [{ id: harness }];
  const harnessName = harnesses.find((h) => h.id === harness)?.name ?? harnessLabel(harness) ?? String(harness);
  const access = ACCESS.find((a) => a.id === sandbox)!;
  const desktopName = connection.desktop ? desktopDisplayName(connection.desktop) : undefined;

  const back = () => { if (router.canGoBack()) router.back(); else router.replace('/'); };

  // Start = create the chat, upload anything attached, dispatch the first turn,
  // and land in the transcript. Same client calls as before the redesign.
  const start = async (text: string) => {
    if (!online || busy) return;
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
      const chatId = await client.createChat(spaceId, s?.deviceId ?? null, { harness, model, reasoning: REASONING, sandbox, modelOptions: {} });
      if (prompt.trim()) {
        await client.run(chatId, { prompt: prompt.trim(), harness, model, reasoning: REASONING, cwd: s?.path ?? '~', sandbox, modelOptions: {}, attachments: paths });
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

  const pickSpace = () => showMenu('Workspace', spaceChoices.map((s) => ({
    label: s.label, checked: s.id === space.id, onPress: () => setSpaceId(s.id),
  })));

  return (
    <Screen>
      <KeyboardAvoidingView style={styles.flex} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
        <TopBar
          left={<CircleButton icon="chevron.left" label="Back" onPress={back} />}
          title="New session"
          subtitle={desktopName}
        />

        <View style={styles.hero}>
          {!online ? (
            <Notice kind="warning" title="Desktop offline" body="Reconnect a desktop to start a session." />
          ) : null}
          <Text accessibilityRole="header" style={[styles.question, { color: t.text }]}>Describe the task</Text>
          <Pressable onPress={pickSpace} accessibilityRole="button" accessibilityLabel={`Workspace, ${space.label}`} style={({ pressed }) => [styles.space, { opacity: pressed ? 0.6 : 1 }]}>
            <Icon name="folder" size={18} color={t.textSecondary} />
            <Text style={[styles.spaceLabel, { color: t.textSecondary }]} numberOfLines={1}>{space.label}</Text>
            <Icon name="chevron.up.chevron.down" size={12} color={t.textTertiary} />
          </Pressable>

          {!typed && online ? (
            <View style={styles.prompts}>
              {PROMPTS.map((p) => (
                <Pressable
                  key={p.label}
                  onPress={() => { void Haptics.selectionAsync(); setDraft(p.label); setTyped(true); }}
                  accessibilityRole="button"
                  accessibilityLabel={p.label}
                  style={({ pressed }) => [styles.prompt, { backgroundColor: t.floating, boxShadow: t.floatShadow, opacity: pressed ? 0.7 : 1 }]}
                >
                  <Icon name={p.icon} size={16} color={t.textSecondary} />
                  <Text style={[type.subhead, { color: t.text }]} numberOfLines={1}>{p.label}</Text>
                  <Icon name="chevron.right" size={11} color={t.textTertiary} weight="semibold" />
                </Pressable>
              ))}
            </View>
          ) : null}
        </View>

        <Composer
          key={draft}
          initialText={draft}
          autoFocus
          placeholder="Describe the task…"
          onChangeText={(v) => setTyped(v.length > 0)}
          running={false}
          sending={busy}
          enabled={online}
          attachments={attachments}
          onAddAttachment={(f) => setAttachments((a) => [...a, f])}
          onRemoveAttachment={(i) => setAttachments((a) => a.filter((_, j) => j !== i))}
          onSend={(text) => void start(text)}
          onStop={() => {}}
          bottomInset={dockInset}
          chips={
            <>
              <ComposerChip
                icon={sandbox === 'danger-full-access' ? 'exclamationmark.shield' : 'lock.shield'}
                tone={sandbox === 'danger-full-access' ? 'warning' : undefined}
                label={access.label}
                accessibilityLabel={`Access, ${access.label}`}
                onPress={() => showMenu('Access', ACCESS.map((a) => ({ label: a.label, checked: a.id === sandbox, onPress: () => setSandbox(a.id) })), ACCESS.map((a) => `${a.label}: ${a.hint}`).join('\n'))}
              />
              <ComposerChip
                leading={<HarnessMark harness={harness} size={17} plain />}
                label={harnessName}
                accessibilityLabel={`Harness, ${harnessName}`}
                onPress={() => showMenu('Harness', harnessChoices.map((h) => {
                  const name = h.name ?? harnessLabel(h.id) ?? String(h.id);
                  return { label: name, checked: h.id === harness, onPress: () => { setHarness(h.id); setModel(null); } };
                }))}
              />
              <ComposerChip
                icon="bolt"
                label={modelName}
                accessibilityLabel={`Model, ${modelName}`}
                onPress={() => showMenu('Model', [
                  { label: 'Default', checked: !model, onPress: () => setModel(null) },
                  ...models.map((m) => ({ label: m.name ?? m.id, checked: m.id === model, onPress: () => setModel(m.id) })),
                ])}
              />
            </>
          }
        />
      </KeyboardAvoidingView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1 },
  hero: { flex: 1, justifyContent: 'center', alignItems: 'center', paddingHorizontal: spacing(6), gap: spacing(2) },
  question: { fontSize: 24, fontWeight: '500', lineHeight: 30, textAlign: 'center', letterSpacing: -0.2 },
  space: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), minHeight: 44, maxWidth: '85%' },
  spaceLabel: { fontSize: 20, lineHeight: 26 },
  prompts: { gap: spacing(2.5), marginTop: spacing(6), alignItems: 'center' },
  prompt: { flexDirection: 'row', alignItems: 'center', gap: spacing(2.5), minHeight: 44, paddingHorizontal: spacing(4), borderRadius: 999 },
});
