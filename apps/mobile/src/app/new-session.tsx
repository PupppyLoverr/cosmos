import React, { useEffect, useMemo, useState } from 'react';
import { ActionSheetIOS, Alert, KeyboardAvoidingView, Platform, Pressable, ScrollView, StyleSheet, Text, TextInput } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { useCosmos, useSnapshot } from '../lib/client';
import { HarnessId, ReasoningLevel, SandboxLevel } from '../lib/protocol';
import { alpha, radii, spacing, type, useTheme } from '../lib/theme';
import { Row, Group, SectionLabel, Button, Segmented, Notice } from '../components/bits';
import { HarnessMark, harnessLabel } from '../components/harness-mark';
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
  const [prompt, setPrompt] = useState(seed ?? '');
  const [busy, setBusy] = useState(false);
  const [focused, setFocused] = useState(false);

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
    () => [{ id: null as string | null, label: 'Desktop', path: undefined as string | undefined }, ...spaces.map((s) => ({ id: s.id as string | null, label: s.name || s.path.split('/').pop() || s.path, path: s.path }))],
    [spaces],
  );
  const space = spaceChoices.find((s) => s.id === spaceId) ?? spaceChoices[0]!;
  const harnessChoices = harnesses.length ? harnesses.filter((h) => h.id !== 'mock') : [{ id: harness }];

  const create = async () => {
    setBusy(true);
    try {
      const s = spaces.find((x) => x.id === spaceId);
      const chatId = await client.createChat(spaceId, s?.deviceId ?? null, { harness, model, reasoning, sandbox, modelOptions: {} });
      if (prompt.trim()) {
        await client.run(chatId, { prompt: prompt.trim(), harness, model, reasoning, cwd: s?.path ?? '~', sandbox, modelOptions: {} });
      }
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      // Deep-link cold opens land here with no screen below, so back() would
      // warn GO_BACK-unhandled; replace puts the session in this slot either way.
      router.replace({ pathname: '/session/[id]', params: { id: chatId, title: prompt.trim().slice(0, 40) || 'Session' } });
    } catch (e) {
      Alert.alert("Couldn't start the session", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Screen>
      <KeyboardAvoidingView style={{ flex: 1 }} behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
        <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.content} keyboardDismissMode="interactive" keyboardShouldPersistTaps="handled">
          {!online ? <Notice kind="warning" title="Desktop offline" body="Link or reconnect a desktop before starting a session." /> : null}

          <Glass radius={radii.xl} style={[styles.promptBox, focused ? { borderColor: alpha(t.accent, 0.6), borderWidth: 1.5 } : { borderColor: t.glassStroke, borderWidth: StyleSheet.hairlineWidth }]}>
            <TextInput
              style={[type.body, styles.prompt, { color: t.text }]}
              autoFocus={!seed}
              placeholder="What should Cosmos do?"
              placeholderTextColor={t.textTertiary}
              selectionColor={t.accent}
              keyboardAppearance={t.scheme}
              multiline
              value={prompt}
              onChangeText={setPrompt}
              onFocus={() => setFocused(true)}
              onBlur={() => setFocused(false)}
              accessibilityLabel="Task for Cosmos"
            />
          </Glass>
          {!prompt.trim() ? (
            <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.ideas} keyboardShouldPersistTaps="handled">
              {PROMPTS.map((p) => (
                <Pressable key={p} onPress={() => { void Haptics.selectionAsync(); setPrompt(p); }} style={({ pressed }) => [styles.idea, { backgroundColor: t.fill, opacity: pressed ? 0.7 : 1 }]} accessibilityRole="button">
                  <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>{p}</Text>
                </Pressable>
              ))}
            </ScrollView>
          ) : null}

          <SectionLabel>Agent</SectionLabel>
          <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.harnesses} keyboardShouldPersistTaps="handled">
            {harnessChoices.map((h) => {
              const on = h.id === harness;
              return (
                <Pressable
                  key={h.id}
                  onPress={() => { void Haptics.selectionAsync(); setHarness(h.id); setModel(null); }}
                  accessibilityRole="radio"
                  accessibilityState={{ selected: on }}
                  accessibilityLabel={harnessLabel(h.id) ?? String(h.id)}
                  style={({ pressed }) => [styles.harness, { backgroundColor: on ? alpha(t.accent, 0.14) : t.surface, borderColor: on ? t.accent : 'transparent', opacity: pressed ? 0.8 : 1 }]}
                >
                  <HarnessMark harness={h.id} size={40} />
                  <Text style={[type.caption, { color: on ? t.link : t.textSecondary, fontWeight: on ? '600' : '500' }]} numberOfLines={1}>{h.name ?? harnessLabel(h.id)}</Text>
                </Pressable>
              );
            })}
          </ScrollView>

          <Group>
            <Row
              title="Model"
              value={modelName}
              icon="cpu"
              iconTint={t.link}
              chevron
              onPress={() => pickFrom('Model', ['Default', ...models.map((m) => m.name ?? m.id)], modelName, (i) => setModel(i === 0 ? null : models[i - 1]!.id))}
            />
            <Row
              title="Project"
              value={space.label}
              subtitle={space.path}
              icon="folder"
              iconTint={t.warning}
              chevron
              onPress={() => pickFrom('Project', spaceChoices.map((s) => s.label), space.label, (i) => setSpaceId(spaceChoices[i]!.id))}
            />
          </Group>

          <SectionLabel>Effort</SectionLabel>
          <Segmented options={REASONING} value={reasoning} onChange={setReasoning} />

          <SectionLabel>Permissions</SectionLabel>
          <Segmented options={SANDBOX.map(({ id, label, icon }) => ({ id, label, icon }))} value={sandbox} onChange={setSandbox} />
          <Text style={[type.footnote, styles.hint, { color: sandbox === 'danger-full-access' ? t.warning : t.textSecondary }]}>
            {SANDBOX.find((s) => s.id === sandbox)!.hint}
          </Text>

          <Button
            title={prompt.trim() ? 'Start' : 'Create Empty Session'}
            icon={prompt.trim() ? 'arrow.up' : undefined}
            variant={prompt.trim() ? 'primary' : 'tinted'}
            onPress={() => void create()}
            loading={busy}
            disabled={!online}
            style={styles.createBtn}
          />
        </ScrollView>
      </KeyboardAvoidingView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  content: { paddingHorizontal: spacing(4), paddingTop: spacing(2), paddingBottom: spacing(12) },
  promptBox: { paddingHorizontal: spacing(4), paddingVertical: spacing(3), marginTop: spacing(2) },
  prompt: { minHeight: 110, textAlignVertical: 'top', lineHeight: 24 },
  ideas: { gap: spacing(2), paddingTop: spacing(3), paddingHorizontal: spacing(0.5) },
  idea: { paddingHorizontal: spacing(3), height: 32, borderRadius: radii.pill, justifyContent: 'center' },
  harnesses: { gap: spacing(2), paddingHorizontal: spacing(0.5), paddingBottom: spacing(3) },
  harness: {
    width: 84, alignItems: 'center', gap: spacing(1.5), paddingVertical: spacing(2.5),
    borderRadius: radii.lg, borderCurve: 'continuous', borderWidth: 1.5,
  },
  hint: { marginTop: spacing(2), marginHorizontal: spacing(2) },
  createBtn: { marginTop: spacing(6) },
});
