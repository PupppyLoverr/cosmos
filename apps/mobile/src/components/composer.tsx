import React, { useState } from 'react';
import { ActionSheetIOS, Alert, KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { SymbolView } from 'expo-symbols';
import * as Haptics from 'expo-haptics';
import { PickedFile, pickPhoto, pickDocument, pickFromClipboard } from '../lib/attachments';
import { radii, spacing, useTheme } from '../lib/theme';
import { Glass } from './surface';

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
      <View style={[styles.wrap, { backgroundColor: t.translucent || t.hasBackdrop ? 'transparent' : t.background }]}>
        {attachments.length > 0 ? (
          <View style={styles.chips}>
            {attachments.map((f, i) => (
              <Glass key={`${f.uri}-${i}`} radius={radii.pill} thin style={styles.chip}>
                <SymbolView name="doc" size={11} tintColor={t.textSecondary} />
                <Text style={{ color: t.textSecondary, fontSize: 12, maxWidth: 140 }} numberOfLines={1}>{f.name}</Text>
                <Pressable onPress={() => onRemoveAttachment(i)} hitSlop={8}>
                  <SymbolView name="xmark" size={11} tintColor={t.textTertiary} />
                </Pressable>
              </Glass>
            ))}
          </View>
        ) : null}
        <Glass radius={26} tint={t.translucent ? undefined : t.composerBg} style={styles.bar}>
          <Pressable
            onPress={openAttachSheet}
            disabled={busy}
            style={({ pressed }) => [styles.iconBtn, { backgroundColor: t.fill, opacity: pressed ? 0.6 : 1 }]}
            accessibilityLabel="Attach"
            accessibilityRole="button"
          >
            <SymbolView name="plus" size={17} tintColor={t.text} weight="medium" />
          </Pressable>
          <TextInput
            style={[styles.input, { color: t.text }]}
            placeholder={placeholder ?? 'Ask Cosmos'}
            placeholderTextColor={t.textTertiary}
            value={text}
            onChangeText={setText}
            multiline
            maxLength={20000}
            submitBehavior="blurAndSubmit"
          />
          {running ? (
            <Pressable onPress={onStop} style={[styles.sendBtn, { backgroundColor: t.accent }]} accessibilityLabel="Stop" accessibilityRole="button">
              <SymbolView name="stop.fill" size={13} tintColor={t.accentContrast} />
            </Pressable>
          ) : (
            <Pressable
              onPress={send}
              disabled={!text.trim() && attachments.length === 0}
              style={[styles.sendBtn, { backgroundColor: t.accent, opacity: !text.trim() && attachments.length === 0 ? 0.3 : 1 }]}
              accessibilityLabel="Send"
              accessibilityRole="button"
            >
              <SymbolView name="arrow.up" size={15} tintColor={t.accentContrast} weight="bold" />
            </Pressable>
          )}
        </Glass>
      </View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  wrap: { borderTopWidth: 0, paddingBottom: spacing(2) },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: spacing(2), paddingHorizontal: spacing(3), paddingTop: spacing(2) },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    paddingHorizontal: spacing(2.5), paddingVertical: spacing(1),
  },
  bar: {
    flexDirection: 'row', alignItems: 'flex-end', gap: spacing(1.5),
    marginHorizontal: spacing(3), marginTop: spacing(2),
    paddingHorizontal: spacing(2), paddingVertical: spacing(2),
  },
  iconBtn: { width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center' },
  input: { flex: 1, fontSize: 17, lineHeight: 22, maxHeight: 140, paddingVertical: spacing(1.5), paddingHorizontal: spacing(1.5) },
  sendBtn: {
    width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center',
  },
});
