import React from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import Animated, { FadeInDown, FadeOutDown } from 'react-native-reanimated';
import { alpha, motion, radii, spacing, type, useTheme } from '../lib/theme';
import { useReduceMotion } from '../lib/motion';
import { Icon, Pulse } from './bits';

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
  const icon = kind === 'error' ? 'exclamationmark.triangle.fill' : kind === 'input' ? 'hand.raised.fill' : undefined;
  const content = (
    <>
      <View style={[styles.iconTile, { backgroundColor: alpha(color, 0.18) }]}>
        {kind === 'controlling' ? <Pulse color={color} /> : <Icon name={icon!} size={15} color={color} />}
      </View>
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
      style={[styles.container, { backgroundColor: alpha(color, 0.12) }]}
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
  container: { minHeight: 52, borderRadius: radii.lg, borderCurve: 'continuous', overflow: 'hidden' },
  pressable: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: spacing(2.5),
    minHeight: 52,
    paddingHorizontal: spacing(3),
    paddingVertical: spacing(2),
  },
  iconTile: { width: 28, height: 28, borderRadius: radii.control, alignItems: 'center', justifyContent: 'center' },
  copy: { flex: 1, minWidth: 0, gap: 1 },
});
