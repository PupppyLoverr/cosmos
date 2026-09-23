import React, { useEffect, useRef, useState } from 'react';
import { ActionSheetIOS, ActivityIndicator, Alert, Platform, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { Image } from 'expo-image';
import * as Haptics from 'expo-haptics';
import Animated, { useAnimatedStyle, useSharedValue, withSpring, withTiming } from 'react-native-reanimated';
import { PickedFile, pickPhoto, pickDocument, pickFromClipboard } from '../lib/attachments';
import { useReduceMotion } from '../lib/motion';
import { motion, radii, spacing, type, useTheme } from '../lib/theme';
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
  /** Focus the field on mount (New session). */
  autoFocus?: boolean;
}

/**
 * Codex-style tool pill in the composer's bottom row: icon + label, no fill.
 * `tone="warning"` for a risky setting (Full access), `leading` for a mark.
 */
export function ComposerChip({ label, icon, leading, tone, onPress, accessibilityLabel, accessibilityHint }: {
  label: string;
  icon?: string;
  leading?: React.ReactNode;
  tone?: 'warning';
  onPress?: () => void;
  accessibilityLabel?: string;
  accessibilityHint?: string;
}) {
  const t = useTheme();
  const fg = tone === 'warning' ? t.warning : t.textSecondary;
  const body = (
    <View style={styles.composerChip}>
      {leading}
      {icon ? <Icon name={icon} size={15} color={fg} /> : null}
      <Text style={[type.subhead, { color: fg, fontWeight: '500', maxWidth: 170 }]} numberOfLines={1}>{label}</Text>
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

/** Photo / file / clipboard picker behind the composer's "+" — shared with the New Session sheet. */
export function useAttachPicker(onAddAttachment: (f: PickedFile) => void): { busy: boolean; open: () => void } {
  const [busy, setBusy] = useState(false);
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
  return { busy, open: openAttachSheet };
}

/** The 36pt filled send circle. Ink on paper; dims when there's nothing to send. */
export function SendButton({ onPress, enabled, sending, label = 'Send' }: { onPress: () => void; enabled: boolean; sending?: boolean; label?: string }) {
  const t = useTheme();
  return (
    <Pressable
      onPress={onPress}
      disabled={!enabled}
      hitSlop={4}
      style={({ pressed }) => [styles.hit, { opacity: pressed ? 0.8 : 1 }]}
      accessibilityLabel={label}
      accessibilityRole="button"
      accessibilityState={{ disabled: !enabled }}
    >
      <View style={[styles.round, { backgroundColor: t.text, opacity: enabled ? 1 : 0.3 }]}>
        {sending ? <ActivityIndicator size="small" color={t.background} /> : <Icon name="arrow.up" size={16} color={t.background} weight="bold" />}
      </View>
    </Pressable>
  );
}

export function Composer({ placeholder, initialText, onChangeText, bottomInset, running, sending, enabled = true, chips, attachments, onAddAttachment, onRemoveAttachment, onSend, onStop, autoFocus }: ComposerProps) {
  const t = useTheme();
  const reduce = useReduceMotion();
  const [text, setTextState] = useState(initialText ?? '');
  const setText = (v: string) => { setTextState(v); onChangeText?.(v); };
  const { busy, open: openAttachSheet } = useAttachPicker(onAddAttachment);
  const [focused, setFocused] = useState(false);
  const input = useRef<TextInput>(null);
  const lift = useSharedValue(0);
  const sendScale = useSharedValue(1);

  // Focus lifts the card: ~8pt of soft shadow and a 2pt rise, 240ms, no bounce.
  useEffect(() => {
    if (reduce) { lift.set(focused ? 1 : 0); return; }
    lift.set(withTiming(focused ? 1 : 0, { duration: motion.axis }));
  }, [focused, reduce, lift]);

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


  const hint = placeholder ?? (running ? 'Steer while it works…' : 'Ask Cosmos');
  const showStop = running && !text.trim() && attachments.length === 0;
  const liftStyle = useAnimatedStyle(() => ({ transform: [{ translateY: -2 * lift.get() }] }));
  const sendAnimStyle = useAnimatedStyle(() => ({ transform: [{ scale: sendScale.get() }] }));
  const overWallpaper = t.hasBackdrop && t.translucent;

  const body = (
    <>
      {attachments.length > 0 ? (
        <View style={styles.chips}>
          {attachments.map((f, i) => <AttachmentChip key={`${f.uri}-${i}`} file={f} onRemove={() => onRemoveAttachment(i)} />)}
        </View>
      ) : null}
      <TextInput
        ref={input}
        style={[styles.input, type.composer, { color: t.text }]}
        placeholder={hint}
        placeholderTextColor={t.textTertiary}
        value={text}
        onChangeText={setText}
        autoFocus={autoFocus || !!initialText}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        multiline
        maxLength={20000}
        keyboardAppearance={t.scheme}
        selectionColor={t.accent}
        submitBehavior="newline"
        accessibilityLabel="Message"
      />
      <View style={styles.toolRow}>
        <Pressable
          onPress={openAttachSheet}
          disabled={busy || sending}
          hitSlop={4}
          style={({ pressed }) => [styles.hit, { opacity: pressed || busy ? 0.6 : 1 }]}
          accessibilityLabel="Attach files"
          accessibilityRole="button"
        >
          {busy ? <ActivityIndicator size="small" color={t.textSecondary} /> : <Icon name="plus" size={22} color={t.text} weight="regular" />}
        </Pressable>
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          keyboardShouldPersistTaps="handled"
          style={styles.chipScroll}
          contentContainerStyle={styles.chipRow}
        >
          {chips}
        </ScrollView>
        {showStop ? (
          <Pressable onPress={stop} style={({ pressed }) => [styles.hit, { opacity: pressed ? 0.75 : 1 }]} accessibilityLabel="Stop the run" accessibilityRole="button">
            <View style={[styles.round, { backgroundColor: t.text }]}>
              <View style={[styles.stop, { backgroundColor: t.background }]} />
            </View>
          </Pressable>
        ) : (
          <Animated.View style={sendAnimStyle}>
            <SendButton onPress={send} enabled={canSend} sending={sending} label={running ? 'Send steering message' : 'Send'} />
          </Animated.View>
        )}
      </View>
    </>
  );

  return (
    <View style={[styles.wrap, bottomInset !== undefined ? { paddingBottom: bottomInset } : null]}>
      {/* Floating card (Codex / Cursor): field on top, tools underneath. */}
      <Animated.View style={liftStyle}>
        {overWallpaper ? (
          <Glass radius={radii.composer} style={styles.card}>{body}</Glass>
        ) : (
          <View style={[styles.card, { backgroundColor: t.floating, boxShadow: t.floatShadow }]}>{body}</View>
        )}
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: { paddingHorizontal: spacing(3), paddingTop: spacing(2), paddingBottom: spacing(2) },
  card: { borderRadius: radii.composer, borderCurve: 'continuous', paddingTop: spacing(1.5), paddingBottom: spacing(1.5), paddingHorizontal: spacing(1.5) },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing(2), paddingHorizontal: spacing(2), paddingTop: spacing(1) },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    paddingLeft: spacing(1), paddingRight: spacing(1.5), paddingVertical: spacing(1),
    borderRadius: radii.row, borderCurve: 'continuous', borderWidth: StyleSheet.hairlineWidth,
  },
  thumb: { width: 32, height: 32, borderRadius: 8, borderCurve: 'continuous' },
  docThumb: { alignItems: 'center', justifyContent: 'center' },
  chipX: { marginLeft: -spacing(0.5) },
  input: {
    minHeight: 44, maxHeight: 160,
    paddingTop: Platform.OS === 'ios' ? 10 : 6, paddingBottom: Platform.OS === 'ios' ? 6 : 4, paddingHorizontal: spacing(2.5),
  },
  toolRow: { flexDirection: 'row', alignItems: 'center' },
  chipScroll: { flex: 1 },
  hit: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  round: { width: 36, height: 36, borderRadius: 18, alignItems: 'center', justifyContent: 'center' },
  stop: { width: 11, height: 11, borderRadius: 3 },
  chipRow: { flexDirection: 'row', alignItems: 'center', gap: spacing(1), paddingHorizontal: spacing(1) },
  composerChip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    minHeight: 36, paddingHorizontal: spacing(2),
    borderRadius: radii.chip,
  },
});
