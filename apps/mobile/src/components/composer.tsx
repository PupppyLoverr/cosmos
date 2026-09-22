import React, { useEffect, useRef, useState } from 'react';
import { ActionSheetIOS, ActivityIndicator, Alert, Platform, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { Image } from 'expo-image';
import * as Haptics from 'expo-haptics';
import Animated, { useAnimatedStyle, useSharedValue, withSpring } from 'react-native-reanimated';
import { PickedFile, pickPhoto, pickDocument, pickFromClipboard } from '../lib/attachments';
import { useReduceMotion } from '../lib/motion';
import { radii, spacing, type, useTheme } from '../lib/theme';
import { Icon } from './bits';
import { Glass } from './surface';

interface ComposerProps {
  placeholder?: string;
  /** Text present when the composer mounts (a suggested prompt). */
  initialText?: string;
  onChangeText?: (text: string) => void;
  /** Extra padding below the bar when it sits at the screen edge. */
  bottomInset?: number;
  /** A turn is in flight: the primary control becomes Stop and text steers the run. */
  running: boolean;
  /** An upload/send is in progress. */
  sending?: boolean;
  /** False while the desktop is unreachable — Stop still works. */
  enabled?: boolean;
  /** Model / checkout chips, the desktop composer's secondary row. */
  chips?: React.ReactNode;
  attachments: PickedFile[];
  onAddAttachment: (f: PickedFile) => void;
  onRemoveAttachment: (index: number) => void;
  onSend: (text: string) => void;
  onStop: () => void;
}

/** Dropdown-style chip for the composer toolbar. 32pt visual, 44pt target. */
export function ComposerChip({ label, icon, onPress, accessibilityLabel, accessibilityHint }: {
  label: string;
  icon?: string;
  onPress?: () => void;
  accessibilityLabel?: string;
  accessibilityHint?: string;
}) {
  const t = useTheme();
  const body = (
    <View style={[styles.composerChip, { borderColor: t.separator, backgroundColor: t.fill }]}>
      {icon ? <Icon name={icon} size={13} color={t.textSecondary} /> : null}
      <Text style={[type.meta, { color: t.text, fontWeight: '500', maxWidth: 160 }]} numberOfLines={1}>{label}</Text>
      {onPress ? <Icon name="chevron.down" size={9} color={t.textTertiary} weight="semibold" /> : null}
    </View>
  );
  if (!onPress) {
    return <View accessibilityLabel={accessibilityLabel ?? label}>{body}</View>;
  }
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel ?? label}
      accessibilityHint={accessibilityHint}
      hitSlop={{ top: 6, bottom: 6 }}
      style={({ pressed }) => ({ opacity: pressed ? 0.65 : 1 })}
    >
      {body}
    </Pressable>
  );
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

export function Composer({ placeholder, initialText, onChangeText, bottomInset, running, sending, enabled = true, chips, attachments, onAddAttachment, onRemoveAttachment, onSend, onStop }: ComposerProps) {
  const t = useTheme();
  const reduce = useReduceMotion();
  const [text, setTextState] = useState(initialText ?? '');
  const setText = (v: string) => { setTextState(v); onChangeText?.(v); };
  const [busy, setBusy] = useState(false);
  const [focused, setFocused] = useState(false);
  const input = useRef<TextInput>(null);
  const halo = useSharedValue(0);
  const sendScale = useSharedValue(1);

  useEffect(() => {
    if (reduce) { halo.set(focused ? 1 : 0); return; }
    halo.set(withSpring(focused ? 1 : 0, { duration: 280, dampingRatio: 0.9 }));
  }, [focused, reduce, halo]);

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

  const canSend = enabled && (text.trim().length > 0 || attachments.length > 0) && !sending;
  const send = () => {
    if (!canSend) return;
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    if (!reduce) sendScale.set(withSpring(0.86, { duration: 120, dampingRatio: 0.6 }, () => sendScale.set(withSpring(1, { duration: 180, dampingRatio: 0.8 }))));
    onSend(text.trim());
    setText('');
  };
  const stop = () => {
    void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning);
    onStop();
  };

  const hint = placeholder ?? (running ? 'Steer while it works…' : 'Do anything…');
  const showStop = running && !text.trim() && attachments.length === 0;
  const haloStyle = useAnimatedStyle(() => ({
    borderColor: `rgba(${t.scheme === 'dark' ? '255,255,255' : '0,0,0'},${0.08 + 0.20 * halo.get()})`,
  }));
  const sendAnimStyle = useAnimatedStyle(() => ({ transform: [{ scale: sendScale.get() }] }));

  return (
    <View style={[styles.wrap, bottomInset !== undefined ? { paddingBottom: bottomInset } : null]}>
      {attachments.length > 0 ? (
        <View style={styles.chips}>
          {attachments.map((f, i) => <AttachmentChip key={`${f.uri}-${i}`} file={f} onRemove={() => onRemoveAttachment(i)} />)}
        </View>
      ) : null}
      {/* The desktop composer is a rounded-rect plate with a hairline rim,
          not a floating glass pill — keep the same silhouette. */}
      <Glass
        radius={radii.composer}
        tint={t.translucent ? undefined : t.composerBg}
        style={[styles.bar, { borderColor: t.separator }]}
      >
        <Animated.View style={[styles.haloWrap, haloStyle]} pointerEvents="none" />
        <View style={styles.inputRow}>
          <Pressable
            onPress={openAttachSheet}
            disabled={busy || sending}
            style={({ pressed }) => [styles.hit, { opacity: pressed || busy ? 0.6 : 1 }]}
            accessibilityLabel="Attach files"
            accessibilityRole="button"
          >
            {busy ? <ActivityIndicator size="small" color={t.textSecondary} /> : <Icon name="paperclip" size={18} color={t.textSecondary} weight="medium" />}
          </Pressable>
          <TextInput
            ref={input}
            style={[styles.input, type.composer, { color: t.text }]}
            placeholder={hint}
            placeholderTextColor={t.textTertiary}
            value={text}
            onChangeText={setText}
            autoFocus={!!initialText}
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
            <Pressable onPress={stop} style={({ pressed }) => [styles.hit, { opacity: pressed ? 0.75 : 1 }]} accessibilityLabel="Stop the run" accessibilityRole="button">
              <View style={[styles.round, { backgroundColor: t.text }]}>
                <View style={[styles.stop, { backgroundColor: t.background }]} />
              </View>
            </Pressable>
          ) : (
            <Pressable
              onPress={send}
              disabled={!canSend}
              style={({ pressed }) => [styles.hit, { opacity: pressed ? 0.8 : 1 }]}
              accessibilityLabel={running ? 'Send steering message' : 'Send'}
              accessibilityRole="button"
              accessibilityState={{ disabled: !canSend }}
            >
              <Animated.View style={sendAnimStyle}>
                <View style={[styles.round, { backgroundColor: t.text, opacity: canSend ? 1 : 0.35 }]}>
                  {sending ? (
                    <ActivityIndicator size="small" color={t.background} />
                  ) : (
                    <Icon name="arrow.up" size={14} color={t.background} weight="bold" />
                  )}
                </View>
              </Animated.View>
            </Pressable>
          )}
        </View>
        {chips ? (
          <ScrollView
            horizontal
            showsHorizontalScrollIndicator={false}
            keyboardShouldPersistTaps="handled"
            contentContainerStyle={styles.chipRow}
          >
            {chips}
          </ScrollView>
        ) : null}
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
    paddingLeft: spacing(1), paddingRight: spacing(1.5), paddingVertical: spacing(1),
    borderWidth: StyleSheet.hairlineWidth,
    overflow: 'hidden',
  },
  haloWrap: { position: 'absolute', top: -1, left: -1, right: -1, bottom: -1, borderRadius: radii.composer + 1, borderWidth: 1.5, borderCurve: 'continuous' },
  inputRow: { flexDirection: 'row', alignItems: 'flex-end' },
  hit: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  round: { width: 28, height: 28, borderRadius: 14, alignItems: 'center', justifyContent: 'center' },
  stop: { width: 11, height: 11, borderRadius: 3 },
  input: { flex: 1, maxHeight: 132, paddingTop: Platform.OS === 'ios' ? 12 : 8, paddingBottom: Platform.OS === 'ios' ? 12 : 8, paddingHorizontal: spacing(1) },
  chipRow: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), paddingLeft: spacing(1), paddingRight: spacing(1), paddingBottom: spacing(1.5) },
  composerChip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1),
    minHeight: 32, paddingHorizontal: spacing(2),
    borderRadius: radii.control, borderCurve: 'continuous', borderWidth: StyleSheet.hairlineWidth,
  },
});
