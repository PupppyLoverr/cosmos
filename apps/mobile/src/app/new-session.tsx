import React, { useEffect, useMemo, useRef, useState } from 'react';
import { ActionSheetIOS, Alert, Platform, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { Image } from 'expo-image';
import { router, useLocalSearchParams } from 'expo-router';
import * as Haptics from 'expo-haptics';
import { useCosmos, useSnapshot } from '../lib/client';
import { HarnessId, ReasoningLevel, SandboxLevel } from '../lib/protocol';
import { PickedFile, ensureDecodable, fileToBase64Chunks } from '../lib/attachments';
import { radii, spacing, type, useTheme } from '../lib/theme';
import { Icon, IconButton, Notice, PromptRow } from '../components/bits';
import { HarnessMark, harnessLabel } from '../components/harness-mark';
import { ComposerChip, SendButton, useAttachPicker } from '../components/composer';

// New Session is a sheet over the Sessions list (Copilot "New Agent Session"
// + Codex "New thread"): one field, a row of chips, a send circle. Effort
// stays at the engine default; it is adjustable per session from the thread.

const REASONING: ReasoningLevel = 'medium';

const ACCESS: { id: SandboxLevel; label: string; hint: string }[] = [
  { id: 'read-only', label: 'Read only', hint: 'Can inspect files but not change them' },
  { id: 'workspace-write', label: 'Workspace', hint: 'Edits stay inside the project folder' },
  { id: 'danger-full-access', label: 'Full access', hint: 'Unrestricted, including Computer Use' },
];

// Prompts that map to things Cosmos actually does on the desktop.
const PROMPTS = [
  'Open the project on the desktop',
  'Check what is on the desktop screen right now',
];

function pickFrom(title: string, options: string[], current: string | undefined, onPick: (index: number) => void, message?: string) {
  if (Platform.OS === 'ios') {
    ActionSheetIOS.showActionSheetWithOptions(
      { title, message, options: [...options.map((o) => (o === current ? `✓ ${o}` : o)), 'Cancel'], cancelButtonIndex: options.length },
      (i) => { if (i < options.length) onPick(i); },
    );
  } else {
    Alert.alert(title, message, [
      ...options.map((o, i) => ({ text: o === current ? `✓ ${o}` : o, onPress: () => onPick(i) })),
      { text: 'Cancel', style: 'cancel' as const },
    ]);
  }
}

function AttachmentThumb({ file, onRemove }: { file: PickedFile; onRemove: () => void }) {
  const t = useTheme();
  const image = file.mimeType?.startsWith('image/') || /\.(png|jpe?g|gif|heic|webp)$/i.test(file.name);
  return (
    <View style={[styles.attachment, { backgroundColor: t.fill }]} accessibilityLabel={`Attachment ${file.name}`}>
      {image
        ? <Image source={{ uri: file.uri }} style={styles.thumb} contentFit="cover" />
        : <Icon name="doc" size={14} color={t.textSecondary} />}
      <Text style={[type.footnote, { color: t.text, maxWidth: 120 }]} numberOfLines={1}>{file.name}</Text>
      <Pressable onPress={onRemove} hitSlop={10} accessibilityRole="button" accessibilityLabel={`Remove ${file.name}`}>
        <Icon name="xmark.circle.fill" size={16} color={t.textTertiary} />
      </Pressable>
    </View>
  );
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
  const [sandbox, setSandbox] = useState<SandboxLevel>('workspace-write');
  const [spaceId, setSpaceId] = useState<string | null>(null);
  const [attachments, setAttachments] = useState<PickedFile[]>([]);
  const [text, setText] = useState(seed ?? '');
  const [busy, setBusy] = useState(false);
  const input = useRef<TextInput>(null);
  const attach = useAttachPicker((f) => setAttachments((a) => [...a, f]));

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
  const modelName = model ? (models.find((m) => m.id === model)?.name ?? model) : 'Default model';

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

  const close = () => { if (router.canGoBack()) router.back(); else router.replace('/'); };
  const canSend = online && !busy && (text.trim().length > 0 || attachments.length > 0);

  // Start = create the chat, upload anything attached, dispatch the first turn,
  // and land in the transcript. Same client calls as before the reskin.
  const start = async () => {
    if (!canSend) return;
    setBusy(true);
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    try {
      const s = spaces.find((x) => x.id === spaceId);
      const paths: string[] = [];
      for (const f of attachments) {
        const n = await ensureDecodable(f);
        paths.push(await client.uploadAttachment(n.name, await fileToBase64Chunks(n)));
      }
      const body = text.trim();
      const prompt = paths.length
        ? `${body}\n\nAttached files (uploaded):\n${paths.map((p) => `- ${p}`).join('\n')}`
        : body;
      const chatId = await client.createChat(spaceId, s?.deviceId ?? null, { harness, model, reasoning: REASONING, sandbox, modelOptions: {} });
      if (prompt.trim()) {
        await client.run(chatId, { prompt: prompt.trim(), harness, model, reasoning: REASONING, cwd: s?.path ?? '~', sandbox, modelOptions: {}, attachments: paths });
      }
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      // Deep-link cold opens land here with no screen below, so back() would
      // warn GO_BACK-unhandled; replace puts the session in this slot either way.
      router.replace({ pathname: '/session/[id]', params: { id: chatId, title: body.slice(0, 40) || 'Session' } });
    } catch (e) {
      Alert.alert("Couldn't start the session", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <View style={[styles.sheet, { backgroundColor: t.background }]}>
      <View style={styles.header}>
        <IconButton icon="xmark" label="Cancel" size={36} tint={t.textSecondary} onPress={close} />
        <Text accessibilityRole="header" style={[type.headline, { color: t.text }]}>New session</Text>
        <SendButton onPress={() => void start()} enabled={canSend} sending={busy} label="Start session" />
      </View>

      <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={styles.content} keyboardDismissMode="interactive">
        {!online ? (
          <Notice kind="warning" title="Desktop offline" body="Reconnect a desktop to start a session." />
        ) : null}

        <TextInput
          ref={input}
          value={text}
          onChangeText={setText}
          autoFocus
          multiline
          maxLength={20000}
          placeholder="What should we do on the desktop?"
          placeholderTextColor={t.textTertiary}
          selectionColor={t.accent}
          keyboardAppearance={t.scheme}
          style={[type.composer, styles.field, { color: t.text }]}
          accessibilityLabel="Task"
        />

        {attachments.length ? (
          <View style={styles.attachments}>
            {attachments.map((f, i) => (
              <AttachmentThumb key={`${f.uri}-${i}`} file={f} onRemove={() => setAttachments((a) => a.filter((_, j) => j !== i))} />
            ))}
          </View>
        ) : null}

        <ScrollView horizontal showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="handled" contentContainerStyle={styles.chips}>
          <ComposerChip icon="plus" label="Attach" accessibilityLabel="Attach files" onPress={attach.open} />
          <Pressable
            onPress={() => pickFrom('Harness', harnessChoices.map((h) => h.name ?? harnessLabel(h.id) ?? String(h.id)), harnessName, (i) => {
              setHarness(harnessChoices[i]!.id);
              setModel(null);
            })}
            accessibilityRole="button"
            accessibilityLabel={`Harness, ${harnessName}`}
            style={({ pressed }) => [styles.harnessChip, { backgroundColor: t.fill, opacity: pressed ? 0.65 : 1 }]}
          >
            <HarnessMark harness={harness} size={18} plain />
            <Text style={[type.footnote, { color: t.text, fontWeight: '500' }]} numberOfLines={1}>{harnessName}</Text>
            <Icon name="chevron.down" size={9} color={t.textTertiary} weight="semibold" />
          </Pressable>
          <ComposerChip
            label={modelName}
            accessibilityLabel={`Model, ${modelName}`}
            onPress={() => pickFrom('Model', ['Default model', ...models.map((m) => m.name ?? m.id)], modelName, (i) => setModel(i === 0 ? null : models[i - 1]!.id))}
          />
          <ComposerChip
            icon="folder"
            label={space.label}
            accessibilityLabel={space.id ? `Workspace, ${space.label}` : 'Current checkout'}
            onPress={() => pickFrom('Workspace', spaceChoices.map((s) => s.label), space.label, (i) => setSpaceId(spaceChoices[i]!.id))}
          />
          <ComposerChip
            icon={sandbox === 'danger-full-access' ? 'exclamationmark.shield' : 'lock.shield'}
            label={access.label}
            accessibilityLabel={`Access, ${access.label}`}
            onPress={() => pickFrom('Access', ACCESS.map((a) => a.label), access.label, (i) => setSandbox(ACCESS[i]!.id), ACCESS.map((a) => `${a.label}: ${a.hint}`).join('\n'))}
          />
        </ScrollView>

        {!text.trim() && online ? (
          <View style={[styles.prompts, { borderTopColor: t.separator }]}>
            {PROMPTS.map((p) => <PromptRow key={p} title={p} onPress={() => { setText(p); input.current?.focus(); }} />)}
          </View>
        ) : null}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  sheet: { flex: 1 },
  header: {
    flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between',
    paddingHorizontal: spacing(3), paddingTop: spacing(4), paddingBottom: spacing(2),
  },
  content: { paddingHorizontal: spacing(4), paddingBottom: spacing(10), gap: spacing(3) },
  field: { minHeight: 120, maxHeight: 260, paddingTop: spacing(2), paddingBottom: spacing(2), textAlignVertical: 'top' },
  attachments: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing(2) },
  attachment: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2), minHeight: 36,
    paddingLeft: spacing(1.5), paddingRight: spacing(2), borderRadius: radii.chip, borderCurve: 'continuous',
  },
  thumb: { width: 26, height: 26, borderRadius: 6 },
  chips: { flexDirection: 'row', alignItems: 'center', gap: spacing(2) },
  harnessChip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    minHeight: 30, paddingHorizontal: spacing(2.5), borderRadius: radii.chip, borderCurve: 'continuous',
  },
  prompts: { marginTop: spacing(2), paddingTop: spacing(2), borderTopWidth: StyleSheet.hairlineWidth },
});
