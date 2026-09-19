import React from 'react';
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';
import { SymbolView } from 'expo-symbols';
import { Theme, radii, spacing, statusColor, useTheme } from '../lib/theme';

export function StatusDot({ status, size = 8 }: { status?: string; size?: number }) {
  const t = useTheme();
  return (
    <View style={{
      width: size, height: size, borderRadius: size / 2,
      backgroundColor: statusColor(t, status),
    }} />
  );
}

export function Avatar({ name, size = 34 }: { name: string; size?: number }) {
  const t = useTheme();
  const initial = (name.trim()[0] ?? '?').toUpperCase();
  return (
    <View style={{
      width: size, height: size, borderRadius: size / 2,
      backgroundColor: t.accentMuted, alignItems: 'center', justifyContent: 'center',
    }}>
      <Text style={{ color: t.accent, fontWeight: '600', fontSize: size * 0.44 }}>{initial}</Text>
    </View>
  );
}

interface RowProps {
  title: string;
  subtitle?: string;
  leading?: React.ReactNode;
  trailing?: React.ReactNode;
  onPress?: () => void;
  onLongPress?: () => void;
  danger?: boolean;
  disabled?: boolean;
  chevron?: boolean;
}

export function Row({ title, subtitle, leading, trailing, onPress, onLongPress, danger, disabled, chevron }: RowProps) {
  const t = useTheme();
  return (
    <Pressable
      onPress={onPress}
      onLongPress={onLongPress}
      disabled={disabled || !onPress}
      style={({ pressed }) => [
        styles.row,
        pressed && onPress ? { backgroundColor: t.surface } : null,
        disabled ? { opacity: 0.5 } : null,
      ]}
      accessibilityRole={onPress ? 'button' : undefined}
    >
      {leading}
      <View style={styles.rowText}>
        <Text style={[styles.rowTitle, { color: danger ? t.danger : t.text }]} numberOfLines={1}>{title}</Text>
        {subtitle ? <Text style={[styles.rowSubtitle, { color: t.textSecondary }]} numberOfLines={2}>{subtitle}</Text> : null}
      </View>
      {trailing}
      {chevron && onPress ? <SymbolView name="chevron.right" size={13} tintColor={t.textTertiary} /> : null}
    </Pressable>
  );
}

export function Group({ children }: { children: React.ReactNode }) {
  const t = useTheme();
  return <View style={[styles.group, { backgroundColor: t.surface, borderColor: t.separator }]}>{children}</View>;
}

export function EmptyState({ icon, title, body, actionLabel, onAction }: {
  icon: string; title: string; body?: string; actionLabel?: string; onAction?: () => void;
}) {
  const t = useTheme();
  return (
    <View style={styles.empty}>
      <SymbolView name={icon as never} size={40} tintColor={t.textTertiary} />
      <Text style={[styles.emptyTitle, { color: t.text }]}>{title}</Text>
      {body ? <Text style={[styles.emptyBody, { color: t.textSecondary }]}>{body}</Text> : null}
      {actionLabel && onAction ? (
        <Pressable onPress={onAction} style={[styles.emptyButton, { backgroundColor: t.accent }]}>
          <Text style={{ color: '#fff', fontWeight: '600' }}>{actionLabel}</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

export function ConnectionBadge({ status, detail }: { status: string; detail?: string }) {
  const t = useTheme();
  const label = status === 'connected' ? 'Connected'
    : status === 'connecting' ? 'Connecting…'
    : status === 'reconnecting' ? 'Reconnecting…'
    : status === 'disconnected' ? 'Offline'
    : (detail ?? 'Error');
  const color = status === 'connected' ? t.success
    : status === 'connecting' || status === 'reconnecting' ? t.warning
    : status === 'disconnected' ? t.textTertiary : t.danger;
  return (
    <View style={[styles.badge, { backgroundColor: t.surface, borderColor: t.separator }]}>
      {status === 'connecting' || status === 'reconnecting'
        ? <ActivityIndicator size={10} color={color} />
        : <View style={[styles.badgeDot, { backgroundColor: color }]} />}
      <Text style={{ color: t.textSecondary, fontSize: 12, fontWeight: '500' }}>{label}</Text>
    </View>
  );
}

export function SectionLabel({ children }: { children: string }) {
  const t = useTheme();
  return <Text style={[styles.section, { color: t.textTertiary }]}>{children.toUpperCase()}</Text>;
}

const styles = StyleSheet.create({
  row: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(4), paddingVertical: spacing(3),
    borderRadius: radii.md,
  },
  rowText: { flex: 1, minWidth: 0 },
  rowTitle: { fontSize: 16, fontWeight: '500' },
  rowSubtitle: { fontSize: 13, marginTop: 1 },
  group: {
    borderRadius: radii.lg, borderWidth: StyleSheet.hairlineWidth, overflow: 'hidden',
  },
  empty: { alignItems: 'center', paddingVertical: spacing(16), gap: spacing(2), paddingHorizontal: spacing(8) },
  emptyTitle: { fontSize: 17, fontWeight: '600', marginTop: spacing(2) },
  emptyBody: { fontSize: 14, textAlign: 'center', lineHeight: 20 },
  emptyButton: { marginTop: spacing(3), paddingHorizontal: spacing(5), paddingVertical: spacing(2.5), borderRadius: radii.pill },
  badge: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    paddingHorizontal: spacing(3), paddingVertical: spacing(1.25),
    borderRadius: radii.pill, borderWidth: StyleSheet.hairlineWidth,
  },
  badgeDot: { width: 7, height: 7, borderRadius: 4 },
  section: { fontSize: 12, fontWeight: '600', letterSpacing: 0.6, marginBottom: spacing(2), marginTop: spacing(5) },
});
