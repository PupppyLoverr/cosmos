// Custom chrome after the Codex / Cursor references: floating circle buttons,
// a flat top bar with a title + subtitle, capsule pills. Replaces the stock
// native large-title headers and tab bar.

import React from 'react';
import { Keyboard, Platform, Pressable, ScrollView, StyleSheet, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { edge, spacing, type, useTheme } from '../lib/theme';
import { HIT_SLOP_44 } from '../lib/a11y';
import { Icon } from './bits';

/**
 * Bottom padding for a composer docked at the screen edge: the home-indicator
 * inset while the keyboard is down, a small gap while it is up.
 */
export function useDockInset(): number {
  const insets = useSafeAreaInsets();
  const [up, setUp] = React.useState(false);
  React.useEffect(() => {
    const show = Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillShow' : 'keyboardDidShow', () => setUp(true));
    const hide = Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillHide' : 'keyboardDidHide', () => setUp(false));
    return () => { show.remove(); hide.remove(); };
  }, []);
  return up ? spacing(2) : Math.max(insets.bottom, spacing(3));
}

/** Plain 20pt icon button with a 44pt hit area. */
export function CircleButton({ icon, label, onPress, size = 44, tint, disabled, badge }: {
  icon: string; label: string; onPress?: () => void; size?: number; tint?: string; disabled?: boolean;
  /** Small accent dot (e.g. "something needs you"). */
  badge?: boolean;
}) {
  const t = useTheme();
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress?.(); }}
      disabled={disabled}
      hitSlop={HIT_SLOP_44(size)}
      accessibilityRole="button"
      accessibilityLabel={label}
      style={({ pressed }) => [
        styles.circle,
        { width: size, height: size },
        { opacity: disabled ? 0.4 : pressed ? 0.5 : 1 },
      ]}
    >
      <Icon name={icon} size={20} color={tint ?? t.textSecondary} weight="medium" />
      {badge ? <View style={[styles.badge, { backgroundColor: t.warning, borderColor: t.floating }]} /> : null}
    </Pressable>
  );
}

/**
 * Screen top bar: left circle, a title (optionally with a subtitle, Codex
 * thread style) and right-hand circles. Sits in flow under the status bar.
 */
export function TopBar({ left, title, subtitle, right, center = false, divider = false, style }: {
  left?: React.ReactNode;
  title?: React.ReactNode;
  subtitle?: string;
  right?: React.ReactNode;
  /** Centre the title (Cowork) instead of leading it (Codex thread). */
  center?: boolean;
  /** Add a themed hairline below the bar when content follows immediately. */
  divider?: boolean;
  style?: StyleProp<ViewStyle>;
}) {
  const t = useTheme();
  const insets = useSafeAreaInsets();
  return (
    <View style={[styles.bar, { paddingTop: insets.top + spacing(1.5), borderBottomWidth: divider ? StyleSheet.hairlineWidth : 0, borderBottomColor: divider ? t.separator : undefined, backgroundColor: divider ? t.background : undefined }, style]}>
      <View style={styles.side}>{left}</View>
      <View style={[styles.titleWrap, center ? styles.centered : null]} accessibilityRole="header">
        {typeof title === 'string' ? (
          <Text style={[type.headline, { color: t.text, fontSize: 17, lineHeight: 21 }]} numberOfLines={1} ellipsizeMode="tail">{title}</Text>
        ) : title}
        {subtitle ? <Text style={[type.meta, { color: t.textSecondary }]} numberOfLines={1}>{subtitle}</Text> : null}
      </View>
      <View style={[styles.side, styles.sideRight]}>{right}</View>
    </View>
  );
}

/** Compact screen title used beneath the top bar. */
export function ScreenTitle({ children, style }: { children: string; style?: StyleProp<ViewStyle> }) {
  const t = useTheme();
  return (
    <View style={[styles.screenTitle, style]}>
      <Text accessibilityRole="header" style={[styles.screenTitleText, { color: t.text }]} numberOfLines={1}>{children}</Text>
    </View>
  );
}

/** Solid ink capsule — the one primary action on a screen ("New", "Pair phone"). */
export function PillButton({ title, icon, onPress, disabled, style, variant = 'ink' }: {
  title: string; icon?: string; onPress: () => void; disabled?: boolean; style?: StyleProp<ViewStyle>;
  /** `ink` = solid primary; `quiet` = floating white capsule (secondary). */
  variant?: 'ink' | 'quiet';
}) {
  const t = useTheme();
  const ink = variant === 'ink';
  const fg = ink ? t.background : t.text;
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={title}
      style={({ pressed }) => [
        styles.pill,
        { backgroundColor: ink ? t.text : t.floating, boxShadow: ink ? undefined : t.floatShadow },
        { opacity: disabled ? 0.4 : pressed ? 0.8 : 1, transform: [{ scale: pressed ? 0.97 : 1 }] },
        style,
      ]}
    >
      {icon ? <Icon name={icon} size={18} color={fg} weight="semibold" /> : null}
      <Text style={[type.headline, { color: fg }]}>{title}</Text>
    </Pressable>
  );
}

/** Capsule host/filter chip (Codex "All · MBP.local · Mac-Studio.local"). */
export function FilterChip({ label, selected, dot, icon, onPress, onLongPress }: {
  label: string; selected?: boolean; dot?: 'online' | 'connecting' | 'offline'; icon?: string;
  onPress?: () => void; onLongPress?: () => void;
}) {
  const t = useTheme();
  const dotColor = dot === 'online' ? t.success : dot === 'connecting' ? t.warning : t.textTertiary;
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress?.(); }}
      onLongPress={onLongPress}
      hitSlop={{ top: 6, bottom: 6 }}
      accessibilityRole="button"
      accessibilityState={{ selected: !!selected }}
      accessibilityLabel={dot ? `${label}, ${dot}` : label}
      style={({ pressed }) => [
        styles.chip,
        { backgroundColor: selected ? t.text : t.surface, opacity: pressed ? 0.7 : 1 },
      ]}
    >
      {dot ? <View style={[styles.dot, { backgroundColor: dotColor }]} /> : null}
      {icon ? <Icon name={icon} size={15} color={selected ? t.background : t.textSecondary} /> : null}
      <Text style={[type.subhead, { color: selected ? t.background : t.text, fontWeight: '500' }]} numberOfLines={1}>{label}</Text>
    </Pressable>
  );
}

export function DeviceStrip({ devices, selected, onSelect, onPair }: {
  devices: { id: string; name: string; dot: 'online' | 'connecting' | 'offline' }[];
  selected: string;
  onSelect: (id: string) => void;
  onPair: () => void;
}) {
  const t = useTheme();
  const dotColor = (dot: 'online' | 'connecting' | 'offline') =>
    dot === 'online' ? t.success : dot === 'connecting' ? t.warning : t.textTertiary;
  const chip = (key: string, label: string | undefined, active: boolean, onPress: () => void, dot?: 'online' | 'connecting' | 'offline', icon?: string) => (
    <Pressable
      key={key}
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      accessibilityRole="button"
      accessibilityState={{ selected: active }}
      accessibilityLabel={label ?? 'Pair a desktop'}
      style={({ pressed }) => [
        styles.deviceChip,
        { backgroundColor: active ? t.text : 'transparent', borderColor: active ? t.text : t.separator, borderWidth: StyleSheet.hairlineWidth, opacity: pressed ? 0.7 : 1 },
      ]}
    >
      {dot ? <View style={[styles.deviceDot, { backgroundColor: dotColor(dot) }]} /> : null}
      {icon ? <Icon name={icon} size={15} color={active ? t.background : t.textSecondary} weight="medium" /> : null}
      {label ? <Text style={[type.subhead, { color: active ? t.background : t.text, fontSize: 13, lineHeight: 16, fontWeight: '500' }]} numberOfLines={1}>{label}</Text> : null}
    </Pressable>
  );
  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      contentContainerStyle={styles.deviceStrip}
      keyboardShouldPersistTaps="handled"
    >
      {chip('all', 'All', selected === 'all', () => onSelect('all'))}
      {devices.map((device) => chip(`device:${device.id}`, device.name, selected === device.id, () => onSelect(device.id), device.dot))}
      {chip('pair', undefined, false, onPair, undefined, 'plus')}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  circle: { alignItems: 'center', justifyContent: 'center' },
  badge: { position: 'absolute', top: 9, right: 9, width: 9, height: 9, borderRadius: 5, borderWidth: 1.5 },
  bar: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), paddingHorizontal: spacing(4), paddingBottom: spacing(2) },
  side: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), minWidth: 44 },
  sideRight: { justifyContent: 'flex-end' },
  titleWrap: { flex: 1, minWidth: 0 },
  centered: { alignItems: 'center' },
  screenTitle: { paddingHorizontal: spacing(4), paddingTop: spacing(3), paddingBottom: spacing(3) },
  screenTitleText: { fontSize: 28, fontWeight: '600', letterSpacing: -0.2, lineHeight: 34 },
  pill: {
    flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: spacing(2),
    minHeight: 52, paddingHorizontal: spacing(6), borderRadius: 999,
  },
  dockPill: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    minHeight: 50, paddingHorizontal: spacing(3.5), borderRadius: 999,
  },
  dockSend: {
    width: 30, height: 30, borderRadius: 15, alignItems: 'center', justifyContent: 'center',
  },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    minHeight: 36, paddingHorizontal: spacing(3.5), borderRadius: 999,
  },
  deviceStrip: { paddingHorizontal: edge, gap: spacing(2), paddingBottom: spacing(1) },
  deviceChip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(2),
    height: 30, paddingHorizontal: spacing(3), borderRadius: 15,
  },
  deviceDot: { width: 6, height: 6, borderRadius: 3 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  sectionLabel: {
    flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between',
    gap: spacing(2), paddingHorizontal: edge, paddingTop: spacing(5), paddingBottom: spacing(1.5),
  },
  sectionLabelCompact: { paddingTop: spacing(2) },
  sectionLabelAligned: { paddingLeft: edge + spacing(5) },
  sectionLabelTitle: { flexDirection: 'row', alignItems: 'center', gap: spacing(2), minWidth: 0, flex: 1 },
});

export function SectionLabel({ children, tone, icon, trailing, compact = false, rowAligned = false }: {
  children: string;
  tone?: 'attention' | 'muted';
  icon?: string;
  trailing?: React.ReactNode;
  compact?: boolean;
  rowAligned?: boolean;
}) {
  const t = useTheme();
  return (
    <View style={[styles.sectionLabel, compact ? styles.sectionLabelCompact : null, rowAligned ? styles.sectionLabelAligned : null]} accessibilityRole="header">
      <View style={styles.sectionLabelTitle}>
        {icon ? <Icon name={icon} size={13} color={t.textTertiary} weight="medium" /> : null}
        <Text style={[type.meta, { color: tone === 'attention' ? t.warning : tone === 'muted' ? t.textTertiary : t.textTertiary, fontWeight: '600' }]} numberOfLines={1}>
          {children}
        </Text>
      </View>
      {trailing !== undefined ? <Text style={[type.caption, { color: t.textTertiary }]}>{trailing}</Text> : null}
    </View>
  );
}

export function DockPill({ placeholder, onPress, disabled, icon = 'plus' }: {
  placeholder: string;
  onPress: () => void;
  disabled?: boolean;
  icon?: string;
}) {
  const t = useTheme();
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={placeholder}
      style={({ pressed }) => [
        styles.dockPill,
        { backgroundColor: t.surface, opacity: disabled ? 0.4 : pressed ? 0.8 : 1 },
      ]}
    >
      <Icon name={icon} size={19} color={t.textSecondary} weight="medium" />
      <Text style={[type.body, { color: t.textTertiary, flex: 1 }]} numberOfLines={1}>{placeholder}</Text>
      <View style={[styles.dockSend, { backgroundColor: t.fill }]}>
        <Icon name="arrow.up" size={15} color={t.textSecondary} weight="semibold" />
      </View>
    </Pressable>
  );
}
