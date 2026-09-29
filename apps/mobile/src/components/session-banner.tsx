import React from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import Animated, { FadeInDown, FadeOutDown } from 'react-native-reanimated';
import { motion, spacing, type, useTheme } from '../lib/theme';
import { useReduceMotion } from '../lib/motion';
import { Pulse } from './bits';

type SessionBannerProps = {
  kind: 'error' | 'input' | 'controlling';
  title: string;
  body?: string;
  onPress?: () => void;
  actions?: React.ReactNode;
};

export function SessionBanner({ kind, title, body, onPress, actions }: SessionBannerProps) {
  const t = useTheme();
  const reduce = useReduceMotion();
  const color = kind === 'error' ? t.danger : kind === 'input' ? t.warning : t.link;
  const content = (
    <>
      {kind === 'controlling' ? <Pulse color={color} /> : <View style={[styles.dot, { backgroundColor: color }]} />}
      <View style={styles.copy}>
        <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]} numberOfLines={1}>{title}</Text>
        {body ? <Text style={[type.footnote, { color: t.textSecondary }]} numberOfLines={1}>{body}</Text> : null}
      </View>
      {actions}
    </>
  );
  const accessibilityRole = onPress ? 'button' : kind === 'error' || kind === 'input' ? 'alert' : undefined;
  return (
    <Animated.View
      entering={reduce ? undefined : FadeInDown.duration(motion.fadeIn)}
      exiting={reduce ? undefined : FadeOutDown}
      style={[styles.container, { borderBottomColor: t.separator }]}
      accessibilityRole={accessibilityRole}
      accessibilityLabel={`${title}${body ? `. ${body}` : ''}`}
    >
      {onPress ? (
        <Pressable
          onPress={onPress}
          accessibilityRole="button"
          accessibilityLabel={`${title}${body ? `. ${body}` : ''}`}
          style={({ pressed }) => [styles.pressable, pressed && { opacity: 0.7 }]}
        >
          {content}
        </Pressable>
      ) : (
        <View style={styles.pressable}>{content}</View>
      )}
    </Animated.View>
  );
}

const styles = StyleSheet.create({
  container: { minHeight: 44, borderBottomWidth: StyleSheet.hairlineWidth, overflow: 'hidden' },
  pressable: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: spacing(1.5),
    minHeight: 44,
    paddingHorizontal: spacing(1.5),
    paddingVertical: spacing(1),
  },
  dot: { width: 6, height: 6, borderRadius: 3 },
  copy: { flex: 1, minWidth: 0, gap: 1 },
});
