import React, { useEffect } from 'react';
import { ActivityIndicator, Pressable, StyleProp, StyleSheet, Text, TextInput, View, ViewStyle, type TextInputProps, type TextStyle } from 'react-native';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import { SymbolView, type SymbolViewProps } from 'expo-symbols';
import { SvgXml } from 'react-native-svg';
import { Host, Slider as NativeSlider } from '@expo/ui';
import * as Haptics from 'expo-haptics';
import { Theme, alpha, edge, motion, radii, shadows, spacing, statusColor, type, useTheme } from '../lib/theme';
import { useReduceMotion } from '../lib/motion';
import { describeLink } from '../lib/connection-state';
import type { ConnectionStatus } from '../lib/rpc';
import { HIT_SLOP_44, useLargeText } from '../lib/a11y';
import { icons, solarFor } from '../lib/icons';
import { SOLAR_ICONS } from '../lib/solar-icons';
import { Card, Glass } from './surface';

// ---------------------------------------------------------------------------
// Icons — the Solar Linear set the desktop embeds (1.5px strokes, 24px
// canvas), tinted to the text color exactly like gpui's SVG renderer. Names
// still arrive as SF Symbol names; `solarFor` aliases them onto the shared
// asset set. An unmapped name falls back to the SF Symbol / Material glyph so
// a one-off symbol never renders blank.

type SFName = Extract<SymbolViewProps['name'], string>;
type SymbolName = Exclude<SymbolViewProps['name'], string>;
type AndroidName = NonNullable<SymbolName['android']>;

export function Icon({ name, size = 17, color, weight = 'medium' }: { name: string; size?: number; color?: string; weight?: Extract<SymbolViewProps['weight'], string> }) {
  const t = useTheme();
  const tint = color ?? t.text;
  const solar = SOLAR_ICONS[solarFor(name) ?? name];
  if (solar) {
    return <SvgXml xml={solar} width={size} height={size} color={tint} />;
  }
  const md = (icons as Record<string, string>)[name];
  const symbol: SymbolViewProps['name'] = md
    ? { ios: name as SFName, android: md as AndroidName, web: md as AndroidName }
    : (name as SFName);
  return <SymbolView name={symbol} size={size} tintColor={tint} weight={weight} resizeMode="scaleAspectFit" />;
}

function PulsingDot({ color, size }: { color: string; size: number }) {
  const phase = useSharedValue(0);
  useEffect(() => {
    phase.set(withRepeat(withTiming(1, { duration: motion.workingPulse, easing: Easing.inOut(Easing.sin) }), -1, true));
  }, [phase]);
  const halo = useAnimatedStyle(() => ({
    opacity: 0.45 * (1 - phase.get()),
    transform: [{ scale: 1 + phase.get() }],
  }));
  return (
    <View style={{ width: size, height: size, alignItems: 'center', justifyContent: 'center' }} accessibilityElementsHidden>
      <Animated.View style={[{ position: 'absolute', width: size, height: size, borderRadius: size / 2, backgroundColor: color }, halo]} />
      <View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: color }} />
    </View>
  );
}

export function StatusDot({ status, size = 8, pulse = false }: { status?: string; size?: number; pulse?: boolean }) {
  const t = useTheme();
  const reduce = useReduceMotion();
  const color = statusColor(t, status);
  if (pulse && !reduce) return <PulsingDot color={color} size={size} />;
  return <View accessibilityElementsHidden style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: color }} />;
}

export function Pulse({ color }: { color: string }) {
  const reduce = useReduceMotion();
  const v = useSharedValue(0);
  useEffect(() => {
    if (reduce) {
      v.set(0);
      return;
    }
    v.set(withRepeat(withTiming(1, { duration: 750, easing: Easing.inOut(Easing.sin) }), -1, true));
  }, [v, reduce]);
  const halo = useAnimatedStyle(() => ({
    opacity: reduce ? 0 : 0.55 - 0.45 * v.get(),
    transform: [{ scale: 1 + (reduce ? 0 : 1.2 * v.get()) }],
  }));
  return (
    <View style={styles.pulse}>
      <Animated.View style={[styles.pulseHalo, { backgroundColor: color }, halo]} />
      <View style={[styles.pulseDot, { backgroundColor: color }]} />
    </View>
  );
}

// ---------------------------------------------------------------------------
// Buttons

// Primary is ink — one solid CTA per screen, never the accent.
const buttonVariants = (t: Theme) => ({
  primary: { bg: t.text, fg: t.background },
  tinted: { bg: t.accentMuted, fg: t.link },
  secondary: { bg: t.fill, fg: t.text },
  ghost: { bg: 'transparent', fg: t.link },
  destructive: { bg: t.danger, fg: '#fff' },
});

export function Button({ title, onPress, variant = 'primary', size = 'lg', loading, disabled, icon, style }: {
  title: string;
  onPress?: () => void;
  variant?: keyof ReturnType<typeof buttonVariants>;
  size?: 'sm' | 'md' | 'lg';
  loading?: boolean;
  disabled?: boolean;
  icon?: string;
  style?: StyleProp<ViewStyle>;
}) {
  const t = useTheme();
  const v = buttonVariants(t)[variant];
  const off = disabled || loading;
  const visual = size === 'lg' ? 50 : size === 'md' ? 44 : 32;
  const pad = HIT_SLOP_44(visual);
  const label = size === 'lg' ? type.headline : size === 'md' ? { ...type.subhead, fontWeight: '600' as const } : { ...type.footnote, fontWeight: '600' as const };
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={title}
      accessibilityState={{ disabled: !!off, busy: !!loading }}
      disabled={off}
      hitSlop={{ top: pad, bottom: pad, left: 4, right: 4 }}
      onPress={() => { void Haptics.selectionAsync(); onPress?.(); }}
      style={({ pressed }) => [
        styles.button,
        size === 'lg' ? styles.buttonLg : size === 'md' ? styles.buttonMd : styles.buttonSm,
        { backgroundColor: v.bg, opacity: disabled ? 0.4 : pressed ? 0.7 : 1 },
        style,
      ]}
    >
      {loading ? <ActivityIndicator color={v.fg} /> : (
        <>
          {icon ? <Icon name={icon} size={size === 'lg' ? 17 : 15} color={v.fg} weight="semibold" /> : null}
          <Text style={[label, { color: v.fg }]}>{title}</Text>
        </>
      )}
    </Pressable>
  );
}

/** Circular chrome button (header actions, composer controls). */
export function IconButton({ icon, onPress, label, size = 36, tint, filled = true, disabled, fill }: {
  icon: string; onPress?: () => void; label: string; size?: number; tint?: string; filled?: boolean; disabled?: boolean;
  /** Solid fill colour (e.g. the accent for a primary action). */
  fill?: string;
}) {
  const t = useTheme();
  const glyph = <Icon name={icon} size={Math.min(17, size * 0.46)} color={tint ?? t.text} weight="semibold" />;
  const box: ViewStyle = { width: size, height: size, alignItems: 'center', justifyContent: 'center' };
  const slop = HIT_SLOP_44(size);
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress?.(); }}
      disabled={disabled}
      hitSlop={slop}
      accessibilityRole="button"
      accessibilityLabel={label}
      style={({ pressed }) => ({ opacity: disabled ? 0.35 : pressed ? 0.6 : 1 })}
    >
      {fill ? (
        <View style={[box, { borderRadius: size / 2, backgroundColor: fill }]}>{glyph}</View>
      ) : filled ? (
        <Glass radius={size / 2} interactive thin style={box}>{glyph}</Glass>
      ) : (
        <View style={box}>{glyph}</View>
      )}
    </Pressable>
  );
}

/** Suggestion / filter capsule. */
export function Chip({ label, icon, onPress, selected }: { label: string; icon?: string; onPress?: () => void; selected?: boolean }) {
  const t = useTheme();
  const fg = selected ? t.accentContrast : t.text;
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress?.(); }}
      accessibilityLabel={label}
      accessibilityRole="button"
      accessibilityState={{ selected: !!selected }}
      hitSlop={{ top: 4, bottom: 4 }}
      style={({ pressed }) => ({ opacity: pressed ? 0.7 : 1 })}
    >
      <View style={[styles.chip, { backgroundColor: selected ? t.accentStrong : t.fill }]}>
        {icon ? <Icon name={icon} size={14} color={selected ? fg : t.link} /> : null}
        <Text style={[type.subhead, { color: fg, fontWeight: '600' }]} numberOfLines={1}>{label}</Text>
      </View>
    </Pressable>
  );
}

/** A suggested prompt: a plain text row, not a card with a sparkle. */
export function PromptRow({ title, onPress }: { title: string; onPress: () => void }) {
  const t = useTheme();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={title}
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      style={({ pressed }) => [styles.promptRow, { opacity: pressed ? 0.55 : 1 }]}
    >
      <Icon name="arrow.turn.down.right" size={14} color={t.textTertiary} />
      <Text style={[type.body, { color: t.textSecondary, flex: 1 }]} numberOfLines={1}>{title}</Text>
    </Pressable>
  );
}

// ---------------------------------------------------------------------------
// Segmented control — the selected segment is a raised, high-contrast pill.

export function Segmented<T extends string>({ options, value, onChange }: {
  options: { id: T; label: string; icon?: string }[]; value: T; onChange: (v: T) => void;
}) {
  const t = useTheme();
  const dark = t.scheme === 'dark';
  return (
    <View style={[styles.segmented, { backgroundColor: dark ? alpha('#FFFFFF', 0.08) : alpha('#000000', 0.06) }]} accessibilityRole="tablist">
      {options.map((o) => {
        const on = o.id === value;
        return (
          <Pressable
            key={o.id}
            onPress={() => { void Haptics.selectionAsync(); onChange(o.id); }}
            accessibilityRole="tab"
            accessibilityState={{ selected: on }}
            style={[styles.segment, on ? { backgroundColor: dark ? t.surfaceRaised : '#FFFFFF', boxShadow: shadows.card } : null]}
          >
            {o.icon ? <Icon name={o.icon} size={13} color={on ? t.text : t.textSecondary} weight="semibold" /> : null}
            <Text style={[type.footnote, { color: on ? t.text : t.textSecondary, fontWeight: on ? '600' : '500' }]} numberOfLines={1}>{o.label}</Text>
          </Pressable>
        );
      })}
    </View>
  );
}

// ---------------------------------------------------------------------------
// Native slider with a real width. `matchContents` on a flexible SwiftUI
// control collapses it to zero width, so the host is sized explicitly.

export function Slider({ value, min = 0, max = 1, step, onChange, label }: {
  value: number; min?: number; max?: number; step?: number; onChange: (v: number) => void; label: string;
}) {
  const t = useTheme();
  return (
    <View style={styles.sliderHost} accessibilityLabel={label}>
      <Host style={StyleSheet.absoluteFill} colorScheme={t.scheme} seedColor={t.accentStrong}>
        <NativeSlider value={value} min={min} max={max} step={step} onValueChange={onChange} />
      </Host>
    </View>
  );
}

// ---------------------------------------------------------------------------
// Lists

export function Tag({ label, color }: { label: string; color?: string }) {
  const t = useTheme();
  return (
    <View style={[styles.tag, { backgroundColor: color ? alpha(color, 0.14) : t.fill }]}>
      <Text style={[type.caption2, { color: color ?? t.textSecondary, fontWeight: '600' }]} numberOfLines={1}>{label}</Text>
    </View>
  );
}

interface RowProps {
  title: string;
  subtitle?: string;
  /** Right-aligned secondary value (iOS "value" cell). */
  value?: string;
  leading?: React.ReactNode;
  /** Shortcut: a tinted SF Symbol tile as the leading element. */
  icon?: string;
  iconTint?: string;
  trailing?: React.ReactNode;
  onPress?: () => void;
  onLongPress?: () => void;
  danger?: boolean;
  disabled?: boolean;
  chevron?: boolean;
  selected?: boolean;
  /** Vertically stack title/subtitle with more breathing room (list items vs settings rows). */
  tall?: boolean;
  accessibilityLabel?: string;
}

export function Row({ title, subtitle, value, leading, icon, iconTint, trailing, onPress, onLongPress, danger, disabled, chevron, selected, tall, accessibilityLabel }: RowProps) {
  const t = useTheme();
  const largeText = useLargeText();
  const lead = leading ?? (icon ? <IconTile name={icon} tint={iconTint} /> : null);
  return (
    <Pressable
      onPress={onPress}
      onLongPress={onLongPress}
      disabled={disabled || (!onPress && !onLongPress)}
      style={({ pressed }) => [
        styles.row,
        tall ? styles.rowTall : null,
        pressed && onPress ? { backgroundColor: t.fill } : null,
        disabled ? { opacity: 0.5 } : null,
      ]}
      accessibilityRole={onPress ? 'button' : undefined}
      accessibilityLabel={accessibilityLabel ?? [title, subtitle, value].filter(Boolean).join(', ')}
      accessibilityState={selected !== undefined ? { selected } : undefined}
    >
      {lead ? <View style={styles.rowLeading}>{lead}</View> : null}
      <View style={value ? styles.rowTextFixed : styles.rowText}>
        <Text style={[type.body, { color: danger ? t.danger : t.text }]} numberOfLines={subtitle && largeText ? 2 : 1}>{title}</Text>
        {subtitle ? <Text style={[type.subhead, { color: t.textSecondary, marginTop: 1 }]} numberOfLines={tall ? 2 : 1}>{subtitle}</Text> : null}
      </View>
      {value ? <Text style={[type.body, styles.rowValue, { color: t.textSecondary }]} numberOfLines={1}>{value}</Text> : null}
      {trailing}
      {selected ? <Icon name="checkmark" size={15} color={t.link} weight="semibold" /> : null}
      {chevron && onPress ? <Icon name="chevron.right" size={13} color={t.textTertiary} weight="semibold" /> : null}
    </Pressable>
  );
}

/** Grouped-form text field: label on the left, value on the right (iOS Settings style). */
export function Field({ label, style, ...input }: { label: string; style?: StyleProp<ViewStyle> } & Omit<TextInputProps, 'style'>) {
  const t = useTheme();
  return (
    <View style={[styles.field, style]}>
      <Text style={[type.body, { color: t.text, minWidth: 72 }]}>{label}</Text>
      <TextInput
        {...input}
        style={[type.body, styles.fieldInput, { color: t.text }]}
        placeholderTextColor={t.textTertiary}
        selectionColor={t.accent}
        keyboardAppearance={t.scheme}
        accessibilityLabel={input.accessibilityLabel ?? label}
      />
    </View>
  );
}

/** Flat mark tile in the desktop idiom: ink glyph on a neutral fill — no
 *  candy-colored tiles. `tint` stays available for genuinely semantic color
 *  (destructive rows); everything else reads monochrome. */
export function IconTile({ name, tint, size = 29 }: { name: string; tint?: string; size?: number }) {
  const t = useTheme();
  const fg = tint ?? t.text;
  const bg = tint ? alpha(tint, 0.14) : t.fill;
  return (
    <View style={{ width: size, height: size, borderRadius: Math.min(8, Math.round(size * 0.28)), backgroundColor: bg, alignItems: 'center', justifyContent: 'center' }}>
      <Icon name={name} size={size * 0.55} color={fg} weight="medium" />
    </View>
  );
}

/** Grouped list (inset grouped on iOS): rows on a surface separated by inset hairlines. */
export function Group({ children, header, footer, insetSeparators = true, style }: {
  children: React.ReactNode; header?: string; footer?: string; insetSeparators?: boolean; style?: StyleProp<ViewStyle>;
}) {
  const t = useTheme();
  const items = React.Children.toArray(children).filter(Boolean);
  return (
    <View style={style}>
      {header ? <SectionLabel>{header}</SectionLabel> : null}
      <Card radius={radii.md}>
        <View style={styles.groupClip}>
          {items.map((child, i) => (
            <React.Fragment key={i}>
              {child}
              {i < items.length - 1 ? <View style={[styles.groupSep, insetSeparators ? styles.groupSepInset : null, { backgroundColor: t.separator }]} /> : null}
            </React.Fragment>
          ))}
        </View>
      </Card>
      {footer ? <Caption style={styles.footer}>{footer}</Caption> : null}
    </View>
  );
}

export function SectionLabel({ children }: { children: string }) {
  return <Caption style={styles.section}>{children}</Caption>;
}

/** Secondary text that stays legible over a photo wallpaper (primary colour + soft shadow). */
export function Caption({ children, style }: { children: React.ReactNode; style?: StyleProp<TextStyle> }) {
  const t = useTheme();
  const over = t.hasBackdrop;
  return (
    <Text
      style={[
        type.footnote,
        { color: over ? t.text : t.textSecondary },
        over ? { textShadowColor: alpha(t.scheme === 'dark' ? '#000000' : '#FFFFFF', 0.8), textShadowRadius: 6, textShadowOffset: { width: 0, height: 0 } } : null,
        style,
      ]}
    >
      {children}
    </Text>
  );
}

// ---------------------------------------------------------------------------
// States

/** Empty state: one line, an optional quiet sentence, one action. No art. */
export function EmptyState({ title, body, actionLabel, onAction, children }: {
  title: string; body?: string; actionLabel?: string; onAction?: () => void; children?: React.ReactNode;
}) {
  const t = useTheme();
  return (
    <View style={styles.empty}>
      <Text accessibilityRole="header" style={[type.subhead, { color: t.textSecondary, textAlign: 'center', fontWeight: '400' }]}>{title}</Text>
      {body ? <Text style={[type.caption, { color: t.textTertiary, textAlign: 'center', marginTop: spacing(1), maxWidth: 300 }]}>{body}</Text> : null}
      {actionLabel && onAction ? (
        <Button title={actionLabel} onPress={onAction} size="md" style={{ marginTop: spacing(4), alignSelf: 'center' }} />
      ) : null}
      {children}
    </View>
  );
}

export function ConnectionBadge({ status, detail }: { status: string; detail?: string }) {
  const t = useTheme();
  const link = describeLink({ status: status as ConnectionStatus, detail });
  const label = link.label;
  const color = link.tone === 'online' ? t.success
    : link.tone === 'pending' ? t.warning
    : link.tone === 'attention' ? t.danger : t.textTertiary;
  return (
    <View style={styles.badge} accessibilityLabel={`Connection: ${label}`}>
      {link.tone === 'pending'
        ? <ActivityIndicator size={10} color={color} />
        : <View style={[styles.badgeDot, { backgroundColor: color }]} />}
      <Text style={[type.footnote, { color: t.textSecondary, fontWeight: '500' }]}>{label}</Text>
    </View>
  );
}

/** Inline, non-blocking notice (info/warning/error) that sits in content flow. */
export function Notice({ kind = 'info', title, body, action, onDismiss }: { kind?: 'info' | 'warning' | 'error'; title: string; body?: string; action?: React.ReactNode; onDismiss?: () => void }) {
  const t = useTheme();
  const color = kind === 'error' ? t.danger : kind === 'warning' ? t.warning : t.link;
  const icon = kind === 'error' ? 'exclamationmark.octagon.fill' : kind === 'warning' ? 'exclamationmark.triangle.fill' : 'info.circle.fill';
  return (
    <View
      accessible={kind === 'info' ? true : undefined}
      accessibilityRole={kind !== 'info' ? 'alert' : undefined}
      accessibilityLabel={`${title}${body ? `. ${body}` : ''}`}
      style={[styles.notice, { backgroundColor: alpha(color, 0.12), borderColor: alpha(color, 0.28) }]}
    >
      <Icon name={icon} size={17} color={color} />
      <View style={{ flex: 1, minWidth: 0, gap: 2 }}>
        <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{title}</Text>
        {body ? <Text style={[type.footnote, { color: t.textSecondary }]}>{body}</Text> : null}
        {action}
      </View>
      {onDismiss ? <IconButton icon="xmark" label="Dismiss" size={28} filled={false} onPress={onDismiss} /> : null}
    </View>
  );
}

export function StatusPill({ status, label, pulse, size = 'sm' }: {
  status: string;
  label: string;
  pulse?: boolean;
  size?: 'sm' | 'md';
}) {
  const t = useTheme();
  const color = statusColor(t, status);
  const textStyle = size === 'md' ? type.footnote : type.meta;
  return (
    <View
      accessibilityRole="text"
      accessibilityLabel={label}
      style={[
        styles.statusPill,
        { backgroundColor: alpha(color, 0.12), minHeight: size === 'md' ? 24 : 20 },
      ]}
    >
      <StatusDot status={status} size={size === 'md' ? 7 : 6} pulse={pulse} />
      <Text maxFontSizeMultiplier={1.6} style={[textStyle, { color, fontWeight: '600' }]}>{label}</Text>
    </View>
  );
}

export type SessionState = 'working' | 'awaitingInput' | 'errored' | 'completed' | 'idle';

const STATE_LABEL: Record<Exclude<SessionState, 'idle'>, string> = {
  working: 'Live', awaitingInput: 'Waiting', errored: 'Error', completed: 'Done',
};

/** Trailing status chip on a session: Live / Waiting / Done / Error. Idle draws nothing. */
export function StatusChip({ state }: { state: SessionState }) {
  const t = useTheme();
  if (state === 'idle') return null;
  const color = state === 'completed' ? t.textSecondary : statusColor(t, state);
  return (
    <View
      accessibilityLabel={STATE_LABEL[state]}
      style={[styles.statusChip, { backgroundColor: state === 'completed' ? t.fill : alpha(color, 0.14) }]}
    >
      {state === 'working' ? <StatusDot status="working" size={6} pulse /> : null}
      <Text maxFontSizeMultiplier={1.6} style={[type.caption, { color, fontWeight: '600' }]}>{STATE_LABEL[state]}</Text>
    </View>
  );
}

/** Host chip (Codex-style): desktop name + presence dot. */
export function DesktopChip({ name, state, onPress }: { name: string; state: 'online' | 'connecting' | 'offline'; onPress?: () => void }) {
  const t = useTheme();
  const status = state === 'online' ? 'connected' : state === 'connecting' ? 'awaitingInput' : undefined;
  return (
    <Pressable
      onPress={onPress}
      disabled={!onPress}
      hitSlop={{ top: 6, bottom: 6 }}
      accessibilityRole={onPress ? 'button' : undefined}
      accessibilityLabel={`${name}, ${state}`}
      style={({ pressed }) => [styles.desktopChip, { backgroundColor: t.fill, opacity: pressed ? 0.6 : 1 }]}
    >
      <StatusDot status={status} size={7} pulse={state === 'connecting'} />
      <Text style={[type.footnote, { color: t.text, fontWeight: '500' }]} numberOfLines={1}>{name}</Text>
    </Pressable>
  );
}

export function SkeletonRows({ count = 5 }: { count?: number }) {
  const t = useTheme();
  return (
    <View style={styles.skeletonList}>
      {Array.from({ length: count }).map((_, i) => (
        <View key={i} style={[styles.skeletonRow, { backgroundColor: t.surface }]}>
          <View style={[styles.skeletonLine, { width: '40%', backgroundColor: t.fill }]} />
          <View style={[styles.skeletonLine, { width: '70%', backgroundColor: t.fill, marginTop: 6 }]} />
          <View style={[styles.skeletonLine, { width: '30%', backgroundColor: t.fill, marginTop: 6 }]} />
        </View>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  button: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: spacing(2), borderCurve: 'continuous' },
  buttonLg: { minHeight: 50, paddingHorizontal: spacing(6), borderRadius: radii.cta },
  buttonMd: { minHeight: 44, paddingHorizontal: spacing(4), borderRadius: radii.md },
  buttonSm: { minHeight: 30, paddingHorizontal: spacing(3), borderRadius: radii.control },
  pulse: { width: 10, height: 10, alignItems: 'center', justifyContent: 'center' },
  pulseHalo: { position: 'absolute', width: 8, height: 8, borderRadius: 4 },
  pulseDot: { width: 7, height: 7, borderRadius: 3.5 },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    paddingHorizontal: spacing(3), minHeight: 32, borderRadius: radii.chip, borderCurve: 'continuous',
  },
  promptRow: { flexDirection: 'row', alignItems: 'center', gap: spacing(2.5), minHeight: 44 },
  segmented: { flexDirection: 'row', padding: 2, borderRadius: radii.sm + 1, borderCurve: 'continuous' },
  segment: { flex: 1, flexDirection: 'row', gap: spacing(1), alignItems: 'center', justifyContent: 'center', minHeight: 44, paddingHorizontal: spacing(1), borderRadius: radii.control, borderCurve: 'continuous' },
  sliderHost: { height: 34, width: '100%' },
  tag: { borderRadius: radii.sm - 3, paddingHorizontal: spacing(1.5), paddingVertical: 2, maxWidth: 140 },
  row: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    paddingHorizontal: edge, paddingVertical: spacing(2.75), minHeight: 44,
  },
  rowTall: { paddingVertical: spacing(3.25), minHeight: 60 },
  rowLeading: { alignItems: 'center', justifyContent: 'center' },
  rowText: { flex: 1, minWidth: 0 },
  // Title keeps its natural width; the value fills the rest and truncates.
  rowTextFixed: { flexShrink: 0 },
  rowValue: { flex: 1, textAlign: 'right' },
  groupClip: { borderRadius: radii.md, overflow: 'hidden' },
  groupSep: { height: StyleSheet.hairlineWidth, marginLeft: edge },
  groupSepInset: { marginLeft: edge + 29 + spacing(3) },
  section: { fontWeight: '500', marginBottom: spacing(1.5), marginTop: spacing(6), marginLeft: edge },
  footer: { marginTop: spacing(1.75), marginHorizontal: edge },
  field: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), paddingHorizontal: edge, minHeight: 46 },
  fieldInput: { flex: 1, textAlign: 'right', paddingVertical: spacing(2.5) },
  empty: { alignItems: 'center', paddingVertical: spacing(16), paddingHorizontal: spacing(8) },
  badge: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5) },
  badgeDot: { width: 7, height: 7, borderRadius: 4 },
  notice: { flexDirection: 'row', gap: spacing(2.5), padding: spacing(3), borderRadius: radii.md, borderCurve: 'continuous', alignItems: 'flex-start', borderWidth: StyleSheet.hairlineWidth },
  statusPill: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5), paddingHorizontal: spacing(2), borderRadius: radii.chip, borderCurve: 'continuous' },
  statusChip: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.25), minHeight: 22, paddingHorizontal: spacing(2), borderRadius: 6, borderCurve: 'continuous' },
  desktopChip: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.75), minHeight: 32, paddingHorizontal: spacing(3), borderRadius: radii.chip, borderCurve: 'continuous' },
  skeletonList: { paddingHorizontal: edge, paddingTop: spacing(3), gap: spacing(2) },
  skeletonRow: { padding: spacing(3), borderRadius: radii.row, borderCurve: 'continuous' },
  skeletonLine: { height: 10, borderRadius: 5, borderCurve: 'continuous' },
});
