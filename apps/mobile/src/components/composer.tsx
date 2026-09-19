import React, { useState } from 'react';
import { ActionSheetIOS, Alert, KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { SymbolView } from 'expo-symbols';
import * as Haptics from 'expo-haptics';
import { PickedFile, pickPhoto, pickDocument, pickFromClipboard } from '../lib/attachments';
import { radii, spacing, useTheme } from '../lib/theme';

interface ComposerProps {
  placeholder?: string;
  running: boolean;
  attachments: PickedFile[];
  onAddAttachment: (f: PickedFile) => void;
  onRemoveAttachment: (index: number) => void;
  onSend: (text: string) => void;
  onStop: () => void;
}

export function Composer({ placeholder, running, attachments, onAddAttachment, onRemoveAttachment, onSend, onStop }: ComposerProps) {
  const t = useTheme();
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);

  const attach = async (which: 'camera' | 'photos' | 'files' | 'clipboard') => {
    setBusy(true);
    try {
      const f = which === 'camera' ? await pickPhoto(true)
        : which === 'photos' ? await pickPhoto(false)
        : which === 'files' ? await pickDocument()
        : await pickFromClipboard();
      if (f) onAddAttachment(f);
    } catch (e) {
      Alert.alert('Attachment failed', e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const openAttachSheet = () => {
    const options = ['Camera', 'Photo Library', 'Files', 'Clipboard Image', 'Cancel'];
    if (Platform.OS === 'ios') {
      ActionSheetIOS.showActionSheetWithOptions(
        { options, cancelButtonIndex: 4 },
        (i) => {
          if (i === 0) void attach('camera');
          if (i === 1) void attach('photos');
          if (i === 2) void attach('files');
          if (i === 3) void attach('clipboard');
        },
      );
    } else {
      Alert.alert('Attach', undefined, [
        { text: 'Camera', onPress: () => void attach('camera') },
        { text: 'Photo Library', onPress: () => void attach('photos') },
        { text: 'Files', onPress: () => void attach('files') },
        { text: 'Clipboard Image', onPress: () => void attach('clipboard') },
        { text: 'Cancel', style: 'cancel' },
      ]);
    }
  };

  const send = () => {
    const body = text.trim();
    if (!body && attachments.length === 0) return;
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    onSend(body);
    setText('');
  };

  return (
    <KeyboardAvoidingView behavior={Platform.OS === 'ios' ? 'padding' : undefined}>
      <View style={[styles.wrap, { borderTopColor: t.separator, backgroundColor: t.background }]}>
        {attachments.length > 0 ? (
          <View style={styles.chips}>
            {attachments.map((f, i) => (
              <View key={`${f.uri}-${i}`} style={[styles.chip, { backgroundColor: t.surface, borderColor: t.separator }]}>
                <SymbolView name="doc" size={11} tintColor={t.textSecondary} />
                <Text style={{ color: t.textSecondary, fontSize: 12, maxWidth: 140 }} numberOfLines={1}>{f.name}</Text>
                <Pressable onPress={() => onRemoveAttachment(i)} hitSlop={8}>
                  <SymbolView name="xmark" size={11} tintColor={t.textTertiary} />
                </Pressable>
              </View>
            ))}
          </View>
        ) : null}
        <View style={[styles.bar, { backgroundColor: t.composerBg }]}>
          <Pressable onPress={openAttachSheet} disabled={busy} style={styles.iconBtn} accessibilityLabel="Attach">
            <SymbolView name="plus" size={20} tintColor={t.textSecondary} />
          </Pressable>
          <TextInput
            style={[styles.input, { color: t.text }]}
            placeholder={placeholder ?? 'Message'}
            placeholderTextColor={t.textTertiary}
            value={text}
            onChangeText={setText}
            multiline
            maxLength={20000}
            submitBehavior="blurAndSubmit"
          />
          {running ? (
            <Pressable onPress={onStop} style={[styles.sendBtn, { backgroundColor: t.danger }]} accessibilityLabel="Stop">
              <SymbolView name="stop.fill" size={15} tintColor="#fff" />
            </Pressable>
          ) : (
            <Pressable
              onPress={send}
              disabled={!text.trim() && attachments.length === 0}
              style={[styles.sendBtn, { backgroundColor: t.accent, opacity: !text.trim() && attachments.length === 0 ? 0.35 : 1 }]}
              accessibilityLabel="Send"
            >
              <SymbolView name="arrow.up" size={15} tintColor="#fff" weight="semibold" />
            </Pressable>
          )}
        </View>
      </View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  wrap: { borderTopWidth: StyleSheet.hairlineWidth, paddingBottom: spacing(2) },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing(2), paddingHorizontal: spacing(3), paddingTop: spacing(2) },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    borderWidth: StyleSheet.hairlineWidth, borderRadius: radii.pill,
    paddingHorizontal: spacing(2.5), paddingVertical: spacing(1),
  },
  bar: {
    flexDirection: 'row', alignItems: 'flex-end', gap: spacing(1),
    margin: spacing(3), borderRadius: radii.xl,
    paddingLeft: spacing(1.5), paddingRight: spacing(1.5), paddingVertical: spacing(1.5),
  },
  iconBtn: { padding: spacing(1.5), marginBottom: 2 },
  input: { flex: 1, fontSize: 16, maxHeight: 140, paddingVertical: spacing(2), paddingHorizontal: spacing(1.5) },
  sendBtn: {
    width: 30, height: 30, borderRadius: 15, alignItems: 'center', justifyContent: 'center', marginBottom: 2,
  },
});
