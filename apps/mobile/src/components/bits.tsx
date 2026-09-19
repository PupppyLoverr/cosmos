import React from 'react';
import { ActivityIndicator, Pressable, StyleProp, StyleSheet, Text, View, ViewStyle } from 'react-native';
import { SymbolView } from 'expo-symbols';
import * as Haptics from 'expo-haptics';
import { Theme, radii, spacing, statusColor, tintFor, type, useTheme } from '../lib/theme';

export function StatusDot({ status, size = 8 }: { status?: string; size?: number }) {
  const t = useTheme();
  return (
    <View style={{
      width: size, height: size, borderRadius: size / 2,
      backgroundColor: statusColor(t, status),
    }} />
  );
}

/** Tinted monogram — the per-item identity mark used in lists and headers. */
export function Avatar({ name, tintKey, size = 36, status }: { name: string; tintKey?: string; size?: number; status?: string }) {
  const t = useTheme();
  const initial = (name.trim()[0] ?? '?').toUpperCase();
  const tint = tintFor(tintKey ?? name);
  return (
    <View style={{ width: size, height: size }}>
      <View style={{
        width: size, height: size, borderRadius: size * 0.34, borderCurve: 'continuous',
        backgroundColor: tint, alignItems: 'center', justifyContent: 'center',
      }}>
        <Text style={{ color: '#fff', fontWeight: '700', fontSize: size * 0.42 }}>{initial}</Text>
      </View>
      {status && status !== 'idle' ? (
        <View style={{
          position: 'absolute', right: -2, bottom: -2, width: 12, height: 12, borderRadius: 6,
          backgroundColor: statusColor(t, status), borderWidth: 2, borderColor: t.background,
        }} />
      ) : null}
    </View>
  );
}

const buttonVariants = (t: Theme) => ({
  primary: { bg: t.accent, fg: t.accentContrast },
  secondary: { bg: t.surface, fg: t.text },
  ghost: { bg: 'transparent', fg: t.textSecondary },
  destructive: { bg: t.danger, fg: '#fff' },
});

export function Button({ title, onPress, variant = 'primary', size = 'lg', loading, disabled, icon, style }: {
  title: string;
  onPress?: () => void;
  variant?: 'primary' | 'secondary' | 'ghost' | 'destructive';
  size?: 'md' | 'lg';
  loading?: boolean;
  disabled?: boolean;
  icon?: string;
  style?: StyleProp<ViewStyle>;
}) {
  const t = useTheme();
  const v = buttonVariants(t)[variant];
  const off = disabled || loading;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={title}
      accessibilityState={{ disabled: !!off, busy: !!loading }}
      disabled={off}
      onPress={() => { void Haptics.selectionAsync(); onPress?.(); }}
      style={({ pressed }) => [
        styles.button,
        size === 'lg' ? styles.buttonLg : styles.buttonMd,
        { backgroundColor: v.bg, opacity: disabled ? 0.4 : pressed ? 0.75 : 1 },
        style,
      ]}
    >
      {loading ? <ActivityIndicator color={v.fg} /> : (
        <>
          {icon ? <SymbolView name={icon as never} size={16} tintColor={v.fg} weight="semibold" /> : null}
          <Text style={[type.headline, { color: v.fg, fontSize: size === 'lg' ? 17 : 15 }]}>{title}</Text>
        </>
      )}
    </Pressable>
  );
}

/** Circular chrome button (header actions, composer controls). */
export function IconButton({ icon, onPress, label, size = 36, tint, filled = true, disabled }: {
  icon: string; onPress?: () => void; label: string; size?: number; tint?: string; filled?: boolean; disabled?: boolean;
}) {
  const t = useTheme();
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      hitSlop={6}
      accessibilityRole="button"
      accessibilityLabel={label}
      style={({ pressed }) => ({
        width: size, height: size, borderRadius: size / 2, alignItems: 'center', justifyContent: 'center',
        backgroundColor: filled ? t.surface : 'transparent',
        opacity: disabled ? 0.4 : pressed ? 0.6 : 1,
      })}
    >
      <SymbolView name={icon as never} size={size * 0.48} tintColor={tint ?? t.text} weight="medium" />
    </Pressable>
  );
}

/** Suggestion / filter capsule. */
export function Chip({ label, sub, icon, onPress, selected }: {
  label: string; sub?: string; icon?: string; onPress?: () => void; selected?: boolean;
}) {
  const t = useTheme();
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      style={({ pressed }) => [
        styles.chip,
        { backgroundColor: selected ? t.accent : t.surface, opacity: pressed ? 0.7 : 1 },
      ]}
    >
      {icon ? <SymbolView name={icon as never} size={14} tintColor={selected ? t.accentContrast : t.textSecondary} /> : null}
      <View style={{ minWidth: 0 }}>
        <Text style={[type.subhead, { color: selected ? t.accentContrast : t.text, fontWeight: '600', lineHeight: 18 }]} numberOfLines={1}>{label}</Text>
        {sub ? <Text style={[type.footnote, { color: selected ? t.accentContrast : t.textSecondary }]} numberOfLines={1}>{sub}</Text> : null}
      </View>
    </Pressable>
  );
}

export function Segmented<T extends string>({ options, value, onChange }: {
  options: { id: T; label: string }[]; value: T; onChange: (v: T) => void;
}) {
  const t = useTheme();
  return (
    <View style={[styles.segmented, { backgroundColor: t.fill }]} accessibilityRole="tablist">
      {options.map((o) => {
        const on = o.id === value;
        return (
          <Pressable
            key={o.id}
            onPress={() => { void Haptics.selectionAsync(); onChange(o.id); }}
            accessibilityRole="tab"
            accessibilityState={{ selected: on }}
            style={[styles.segment, on ? { backgroundColor: t.surfaceRaised, boxShadow: '0 1px 3px rgba(0,0,0,0.12)' } : null]}
          >
            <Text style={[type.footnote, { color: on ? t.text : t.textSecondary, fontWeight: on ? '600' : '500' }]}>{o.label}</Text>
          </Pressable>
        );
      })}
    </View>
  );
}

export function Tag({ label, color }: { label: string; color?: string }) {
  const t = useTheme();
  return (
    <View style={[styles.tag, { backgroundColor: color ? `${color}22` : t.surface }]}>
      <Text style={[type.caption, { color: color ?? t.textSecondary }]} numberOfLines={1}>{label}</Text>
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
        pressed && onPress ? { backgroundColor: t.fill } : null,
        disabled ? { opacity: 0.5 } : null,
      ]}
      accessibilityRole={onPress ? 'button' : undefined}
    >
      {leading ? <View style={styles.rowLeading}>{leading}</View> : null}
      <View style={styles.rowText}>
        <Text style={[type.callout, { color: danger ? t.danger : t.text, fontWeight: '500' }]} numberOfLines={1}>{title}</Text>
        {subtitle ? <Text style={[type.footnote, { color: t.textSecondary, marginTop: 1 }]} numberOfLines={2}>{subtitle}</Text> : null}
      </View>
      {trailing}
      {chevron && onPress ? <SymbolView name="chevron.right" size={13} tintColor={t.textTertiary} weight="semibold" /> : null}
    </Pressable>
  );
}

/** Grouped list — rows separated by inset hairlines, not borders. */
export function Group({ children }: { children: React.ReactNode }) {
  const t = useTheme();
  const items = React.Children.toArray(children).filter(Boolean);
  return (
    <View style={[styles.group, { backgroundColor: t.surface }]}>
      {items.map((child, i) => (
        <React.Fragment key={i}>
          {child}
          {i < items.length - 1 ? <View style={[styles.groupSep, { backgroundColor: t.separator }]} /> : null}
        </React.Fragment>
      ))}
    </View>
  );
}

export function EmptyState({ icon, title, body, actionLabel, onAction, children }: {
  icon: string; title: string; body?: string; actionLabel?: string; onAction?: () => void; children?: React.ReactNode;
}) {
  const t = useTheme();
  return (
    <View style={styles.empty}>
      <View style={[styles.emptyGlyph, { backgroundColor: t.surface }]}>
        <SymbolView name={icon as never} size={30} tintColor={t.textTertiary} weight="light" />
      </View>
      <Text style={[type.title2, { color: t.text, marginTop: spacing(2) }]}>{title}</Text>
      {body ? <Text style={[type.subhead, { color: t.textSecondary, textAlign: 'center' }]}>{body}</Text> : null}
      {actionLabel && onAction ? (
        <Button title={actionLabel} onPress={onAction} size="md" style={{ marginTop: spacing(3), alignSelf: 'center', paddingHorizontal: spacing(6) }} />
      ) : null}
      {children}
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
    <View style={[styles.badge, { backgroundColor: t.surface }]}>
      {status === 'connecting' || status === 'reconnecting'
        ? <ActivityIndicator size={10} color={color} />
        : <View style={[styles.badgeDot, { backgroundColor: color }]} />}
      <Text style={[type.caption, { color: t.textSecondary }]}>{label}</Text>
    </View>
  );
}

export function SectionLabel({ children }: { children: string }) {
  const t = useTheme();
  return <Text style={[styles.section, { color: t.textSecondary }]}>{children}</Text>;
}

const styles = StyleSheet.create({
  button: {
    flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: spacing(2),
    borderRadius: radii.pill,
  },
  buttonLg: { paddingVertical: spacing(3.75), paddingHorizontal: spacing(6) },
  buttonMd: { paddingVertical: spacing(2.5), paddingHorizontal: spacing(4) },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    paddingHorizontal: spacing(3.5), paddingVertical: spacing(2.25),
    borderRadius: radii.lg, borderCurve: 'continuous',
  },
  segmented: { flexDirection: 'row', padding: 2, borderRadius: radii.sm + 1, borderCurve: 'continuous' },
  segment: { flex: 1, alignItems: 'center', paddingVertical: spacing(1.5), borderRadius: radii.sm - 1, borderCurve: 'continuous' },
  tag: { borderRadius: radii.sm - 2, paddingHorizontal: spacing(1.5), paddingVertical: 2, maxWidth: 120 },
  row: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: spacing(4), paddingVertical: spacing(3), minHeight: 52,
  },
  rowLeading: { width: 28, alignItems: 'center' },
  rowText: { flex: 1, minWidth: 0 },
  group: { borderRadius: radii.lg, borderCurve: 'continuous', overflow: 'hidden' },
  groupSep: { height: StyleSheet.hairlineWidth, marginLeft: spacing(4) + 28 + spacing(3) },
  empty: { alignItems: 'center', paddingVertical: spacing(14), gap: spacing(2), paddingHorizontal: spacing(8) },
  emptyGlyph: { width: 72, height: 72, borderRadius: 36, alignItems: 'center', justifyContent: 'center' },
  badge: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    paddingHorizontal: spacing(2.5), paddingVertical: spacing(1.25),
    borderRadius: radii.pill,
  },
  badgeDot: { width: 7, height: 7, borderRadius: 4 },
  section: { fontSize: 13, fontWeight: '600', marginBottom: spacing(2), marginTop: spacing(6), marginLeft: spacing(1) },
});
