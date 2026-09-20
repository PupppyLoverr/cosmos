import React, { useRef, useState } from 'react';
import { ActionSheetIOS, ActivityIndicator, Alert, Platform, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { Image } from 'expo-image';
import * as Haptics from 'expo-haptics';
import { PickedFile, pickPhoto, pickDocument, pickFromClipboard } from '../lib/attachments';
import { alpha, radii, spacing, type, useTheme } from '../lib/theme';
import { Icon } from './bits';
import { Glass } from './surface';

interface ComposerProps {
  placeholder?: string;
  /** A turn is in flight: the primary control becomes Stop and text steers the run. */
  running: boolean;
  /** An upload/send is in progress. */
  sending?: boolean;
  attachments: PickedFile[];
  onAddAttachment: (f: PickedFile) => void;
  onRemoveAttachment: (index: number) => void;
  onSend: (text: string) => void;
  onStop: () => void;
}

type Source = 'camera' | 'photos' | 'files' | 'clipboard';
const SOURCES: { id: Source; label: string }[] = [
  { id: 'camera', label: 'Take Photo' },
  { id: 'photos', label: 'Photo Library' },
  { id: 'files', label: 'Choose File' },
  { id: 'clipboard', label: 'Paste Image' },
];

function isImage(f: PickedFile) {
  return f.mimeType?.startsWith('image/') || /\.(png|jpe?g|gif|heic|webp)$/i.test(f.name);
}

function AttachmentChip({ file, onRemove }: { file: PickedFile; onRemove: () => void }) {
  const t = useTheme();
  const image = isImage(file);
  return (
    <View style={[styles.chip, { backgroundColor: t.surface, borderColor: t.separator }]} accessibilityLabel={`Attachment ${file.name}`}>
      {image ? (
        <Image source={{ uri: file.uri }} style={styles.thumb} contentFit="cover" transition={120} />
      ) : (
        <View style={[styles.thumb, styles.docThumb, { backgroundColor: t.fill }]}>
          <Icon name="doc" size={14} color={t.textSecondary} />
        </View>
      )}
      <Text style={[type.footnote, { color: t.text, maxWidth: 120 }]} numberOfLines={1}>{file.name}</Text>
      <Pressable onPress={onRemove} hitSlop={10} accessibilityRole="button" accessibilityLabel={`Remove ${file.name}`} style={styles.chipX}>
        <Icon name="xmark.circle.fill" size={16} color={t.textTertiary} />
      </Pressable>
    </View>
  );
}

export function Composer({ placeholder, running, sending, attachments, onAddAttachment, onRemoveAttachment, onSend, onStop }: ComposerProps) {
  const t = useTheme();
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [focused, setFocused] = useState(false);
  const input = useRef<TextInput>(null);

  const attach = async (which: Source) => {
    setBusy(true);
    try {
      const f = which === 'camera' ? await pickPhoto(true)
        : which === 'photos' ? await pickPhoto(false)
        : which === 'files' ? await pickDocument()
        : await pickFromClipboard();
      if (f) {
        onAddAttachment(f);
        void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
      }
    } catch (e) {
      Alert.alert("Couldn't attach that", e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const openAttachSheet = () => {
    if (Platform.OS === 'ios') {
      ActionSheetIOS.showActionSheetWithOptions(
        { options: [...SOURCES.map((s) => s.label), 'Cancel'], cancelButtonIndex: SOURCES.length, title: 'Add to message' },
        (i) => { const s = SOURCES[i]; if (s) void attach(s.id); },
      );
    } else {
      Alert.alert('Add to message', undefined, [
        ...SOURCES.map((s) => ({ text: s.label, onPress: () => void attach(s.id) })),
        { text: 'Cancel', style: 'cancel' as const },
      ]);
    }
  };

  const canSend = (text.trim().length > 0 || attachments.length > 0) && !sending;
  const send = () => {
    if (!canSend) return;
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    onSend(text.trim());
    setText('');
  };
  const stop = () => {
    void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning);
    onStop();
  };

  const hint = placeholder ?? (running ? 'Steer while it works…' : 'Message Cosmos');
  const showStop = running && !text.trim() && attachments.length === 0;

  return (
    <View style={styles.wrap}>
      {attachments.length > 0 ? (
        <View style={styles.chips}>
          {attachments.map((f, i) => <AttachmentChip key={`${f.uri}-${i}`} file={f} onRemove={() => onRemoveAttachment(i)} />)}
        </View>
      ) : null}
      <Glass
        radius={26}
        tint={t.translucent ? undefined : t.composerBg}
        style={[styles.bar, { borderColor: focused ? alpha(t.accent, 0.45) : t.glassStroke }]}
      >
        <Pressable
          onPress={openAttachSheet}
          disabled={busy || sending}
          hitSlop={6}
          style={({ pressed }) => [styles.round, { backgroundColor: t.fill, opacity: pressed || busy ? 0.6 : 1 }]}
          accessibilityLabel="Add attachment"
          accessibilityRole="button"
        >
          {busy ? <ActivityIndicator size="small" color={t.textSecondary} style={{ transform: [{ scale: 0.8 }] }} /> : <Icon name="plus" size={18} color={t.text} weight="medium" />}
        </Pressable>
        <TextInput
          ref={input}
          style={[styles.input, type.body, { color: t.text }]}
          placeholder={hint}
          placeholderTextColor={t.textTertiary}
          value={text}
          onChangeText={setText}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          multiline
          maxLength={20000}
          keyboardAppearance={t.scheme}
          selectionColor={t.accent}
          submitBehavior="newline"
          accessibilityLabel="Message"
        />
        {showStop ? (
          <Pressable onPress={stop} hitSlop={6} style={({ pressed }) => [styles.round, { backgroundColor: t.text, opacity: pressed ? 0.75 : 1 }]} accessibilityLabel="Stop the run" accessibilityRole="button">
            <Icon name="stop.fill" size={13} color={t.background} />
          </Pressable>
        ) : (
          <Pressable
            onPress={send}
            disabled={!canSend}
            hitSlop={6}
            style={({ pressed }) => [styles.round, { backgroundColor: canSend ? t.accentStrong : t.fill, opacity: pressed ? 0.8 : 1 }]}
            accessibilityLabel={running ? 'Send steering message' : 'Send'}
            accessibilityRole="button"
            accessibilityState={{ disabled: !canSend }}
          >
            {sending ? (
              <ActivityIndicator size="small" color={t.accentContrast} style={{ transform: [{ scale: 0.8 }] }} />
            ) : (
              <Icon name="arrow.up" size={16} color={canSend ? t.accentContrast : t.textTertiary} weight="bold" />
            )}
          </Pressable>
        )}
      </Glass>
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: { paddingHorizontal: spacing(3), paddingTop: spacing(1.5), paddingBottom: spacing(2) },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing(2), paddingHorizontal: spacing(1), paddingBottom: spacing(2) },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    paddingLeft: spacing(1), paddingRight: spacing(1.5), paddingVertical: spacing(1),
    borderRadius: radii.md, borderCurve: 'continuous', borderWidth: StyleSheet.hairlineWidth,
  },
  thumb: { width: 32, height: 32, borderRadius: 8, borderCurve: 'continuous' },
  docThumb: { alignItems: 'center', justifyContent: 'center' },
  chipX: { marginLeft: -spacing(0.5) },
  bar: {
    flexDirection: 'row', alignItems: 'flex-end', gap: spacing(1.5),
    paddingHorizontal: spacing(1.5), paddingVertical: spacing(1.5),
    borderWidth: StyleSheet.hairlineWidth,
  },
  round: { width: 36, height: 36, borderRadius: 18, alignItems: 'center', justifyContent: 'center' },
  input: { flex: 1, maxHeight: 132, paddingVertical: Platform.OS === 'ios' ? spacing(2) : spacing(1.25), paddingHorizontal: spacing(1.5), lineHeight: 22 },
});
