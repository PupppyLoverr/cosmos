import React, { useEffect, useMemo, useState } from 'react';
import { Alert, ScrollView, StyleSheet, TextInput } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useCosmos, useSnapshot } from '../lib/client';
import { HarnessId, ReasoningLevel, SandboxLevel } from '../lib/protocol';
import { useTheme, spacing, radii } from '../lib/theme';
import { Row, Group, SectionLabel, Button } from '../components/bits';
import { Glass, Screen } from '../components/surface';

const REASONING: { id: ReasoningLevel; label: string }[] = [
  { id: 'low', label: 'Low' },
  { id: 'medium', label: 'Medium' },
  { id: 'high', label: 'High' },
];

const SANDBOX: { id: SandboxLevel; label: string }[] = [
  { id: 'read-only', label: 'Read only' },
  { id: 'workspace-write', label: 'Workspace' },
  { id: 'danger-full-access', label: 'Full access' },
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
  const [reasoning, setReasoning] = useState<ReasoningLevel>('medium');
  const [sandbox, setSandbox] = useState<SandboxLevel>('workspace-write');
  const [spaceId, setSpaceId] = useState<string | null>(null);
  const [prompt, setPrompt] = useState(seed ?? '');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (connection.status !== 'connected') return;
    void client.listHarnesses().then(setHarnesses).catch(() => {});
  }, [client, connection.status]);

  useEffect(() => {
    if (connection.status !== 'connected' || !harness) return;
    let cancelled = false;
    void client
      .listModels(harness)
      .then((list) => { if (!cancelled) setModelsFor({ harness, list }); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [client, connection.status, harness]);
  const models = modelsFor.harness === harness ? modelsFor.list : [];

  const spaceChoices = useMemo(
    () => [{ id: null as string | null, label: 'Desktop (no project)' }, ...spaces.map((s) => ({ id: s.id as string | null, label: s.name || s.path.split('/').pop() || s.path }))],
    [spaces],
  );

  const create = async () => {
    setBusy(true);
    try {
      const space = spaces.find((s) => s.id === spaceId);
      const chatId = await client.createChat(spaceId, space?.deviceId ?? null, {
        harness,
        model,
        reasoning,
        sandbox,
        modelOptions: {},
      });
      if (prompt.trim()) {
        await client.run(chatId, {
          prompt: prompt.trim(),
          harness,
          model,
          reasoning,
          cwd: space?.path ?? '~',
          sandbox,
          modelOptions: {},
        });
      }
      // Deep-link cold opens land here with no screen below, so back() would
      // warn GO_BACK-unhandled; replace puts the session in this slot either way.
      router.replace({ pathname: '/session/[id]', params: { id: chatId, title: prompt.trim().slice(0, 40) || 'Session' } });
    } catch (e) {
      Alert.alert('Could not create session', e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const pickList = (title: string, options: string[], current: string, onPick: (v: string) => void) => {
    Alert.alert(title, undefined, [
      ...options.map((o) => ({ text: o === current ? `✓ ${o}` : o, onPress: () => onPick(o) })),
      { text: 'Cancel', style: 'cancel' as const },
    ]);
  };

  return (
    <Screen>
      <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={{ padding: spacing(4), paddingBottom: spacing(12) }}>
        <Glass radius={radii.xl} style={styles.promptBox}>
          <TextInput
            style={{ color: t.text, fontSize: 17, lineHeight: 23, minHeight: 96, textAlignVertical: 'top' }}
            autoFocus={!seed}
            placeholder="What should Cosmos do?"
            placeholderTextColor={t.textTertiary}
            multiline
            value={prompt}
            onChangeText={setPrompt}
          />
        </Glass>

        <SectionLabel>Project</SectionLabel>
        <Group>
          <Row
            title={spaceChoices.find((s) => s.id === spaceId)?.label ?? 'Desktop (no project)'}
            subtitle={spaceId ? spaces.find((s) => s.id === spaceId)?.path : undefined}
            leading={<SymbolView name="folder" size={18} tintColor={t.textSecondary} />}
            chevron
            onPress={() =>
              pickList('Project', spaceChoices.map((s) => s.label),
                spaceChoices.find((s) => s.id === spaceId)?.label ?? '',
                (label) => setSpaceId(spaceChoices.find((s) => s.label === label)?.id ?? null))
            }
          />
        </Group>

        <SectionLabel>Agent</SectionLabel>
        <Group>
          <Row
            title="Harness"
            subtitle={harness}
            leading={<SymbolView name="cpu" size={18} tintColor={t.textSecondary} />}
            chevron
            onPress={() => pickList('Harness', harnesses.length ? harnesses.map((h) => String(h.id)) : [harness], harness, (v) => setHarness(v as HarnessId))}
          />
          <Row
            title="Model"
            subtitle={model ?? 'Default'}
            leading={<SymbolView name="brain" size={18} tintColor={t.textSecondary} />}
            chevron
            onPress={() => pickList('Model', ['Default', ...models.map((m) => m.name ?? m.id)], model ?? 'Default', (v) => setModel(v === 'Default' ? null : models.find((m) => (m.name ?? m.id) === v)?.id ?? v))}
          />
          <Row
            title="Effort"
            subtitle={reasoning}
            leading={<SymbolView name="gauge.with.dots.needle.33percent" size={18} tintColor={t.textSecondary} />}
            chevron
            onPress={() => pickList('Effort', REASONING.map((r) => r.label), REASONING.find((r) => r.id === reasoning)!.label, (v) => setReasoning(REASONING.find((r) => r.label === v)!.id))}
          />
          <Row
            title="Sandbox"
            subtitle={SANDBOX.find((s) => s.id === sandbox)!.label}
            leading={<SymbolView name="lock.shield" size={18} tintColor={t.textSecondary} />}
            chevron
            onPress={() => pickList('Sandbox', SANDBOX.map((s) => s.label), SANDBOX.find((s) => s.id === sandbox)!.label, (v) => setSandbox(SANDBOX.find((s) => s.label === v)!.id))}
          />
        </Group>

        <Button
          title={prompt.trim() ? 'Start session' : 'Create session'}
          icon="arrow.up"
          onPress={() => void create()}
          loading={busy}
          disabled={connection.status !== 'connected'}
          style={styles.createBtn}
        />
      </ScrollView>
    </Screen>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  promptBox: {
    paddingHorizontal: spacing(4), paddingVertical: spacing(3), marginTop: spacing(2),
  },
  createBtn: { marginTop: spacing(6) },
});
