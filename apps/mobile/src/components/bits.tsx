import React from 'react';
import { ActivityIndicator, Pressable, StyleProp, StyleSheet, Text, TextInput, View, ViewStyle, type TextInputProps, type TextStyle } from 'react-native';
import { SymbolView, type SymbolViewProps } from 'expo-symbols';
import { SvgXml } from 'react-native-svg';
import { Host, Slider as NativeSlider } from '@expo/ui';
import * as Haptics from 'expo-haptics';
import { Theme, alpha, edge, radii, shadows, spacing, statusColor, type, useTheme } from '../lib/theme';
import { icons, solarFor } from '../lib/icons';
import { SOLAR_ICONS } from '../lib/solar-icons';
import { Glass } from './surface';

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

export function StatusDot({ status, size = 8 }: { status?: string; size?: number }) {
  const t = useTheme();
  return <View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: statusColor(t, status) }} />;
}

// ---------------------------------------------------------------------------
// Buttons

const buttonVariants = (t: Theme) => ({
  primary: { bg: t.accentStrong, fg: t.accentContrast },
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
  const label = size === 'lg' ? type.headline : size === 'md' ? { ...type.subhead, fontWeight: '600' as const } : { ...type.footnote, fontWeight: '600' as const };
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={title}
      accessibilityState={{ disabled: !!off, busy: !!loading }}
      disabled={off}
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
  const glyph = <Icon name={icon} size={size * 0.46} color={tint ?? t.text} weight="semibold" />;
  const box: ViewStyle = { width: size, height: size, alignItems: 'center', justifyContent: 'center' };
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress?.(); }}
      disabled={disabled}
      hitSlop={6}
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
      accessibilityRole="button"
      accessibilityState={{ selected: !!selected }}
      style={({ pressed }) => ({ opacity: pressed ? 0.7 : 1 })}
    >
      <View style={[styles.chip, { backgroundColor: selected ? t.accentStrong : t.fill }]}>
        {icon ? <Icon name={icon} size={14} color={selected ? fg : t.link} /> : null}
        <Text style={[type.subhead, { color: fg, fontWeight: '600' }]} numberOfLines={1}>{label}</Text>
      </View>
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
}

export function Row({ title, subtitle, value, leading, icon, iconTint, trailing, onPress, onLongPress, danger, disabled, chevron, selected, tall }: RowProps) {
  const t = useTheme();
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
      accessibilityState={selected !== undefined ? { selected } : undefined}
    >
      {lead ? <View style={styles.rowLeading}>{lead}</View> : null}
      <View style={value ? styles.rowTextFixed : styles.rowText}>
        <Text style={[type.body, { color: danger ? t.danger : t.text }]} numberOfLines={1}>{title}</Text>
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
      <Glass radius={radii.md}>
        <View style={styles.groupClip}>
          {items.map((child, i) => (
            <React.Fragment key={i}>
              {child}
              {i < items.length - 1 ? <View style={[styles.groupSep, insetSeparators ? styles.groupSepInset : null, { backgroundColor: t.separator }]} /> : null}
            </React.Fragment>
          ))}
        </View>
      </Glass>
      {footer ? <Caption style={styles.footer}>{footer}</Caption> : null}
    </View>
  );
}

export function SectionLabel({ children }: { children: string }) {
  return <Caption style={styles.section}>{children.toUpperCase()}</Caption>;
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

export function EmptyState({ icon, title, body, actionLabel, onAction, children }: {
  icon: string; title: string; body?: string; actionLabel?: string; onAction?: () => void; children?: React.ReactNode;
}) {
  const t = useTheme();
  return (
    <View style={styles.empty}>
      <View style={[styles.emptyGlyph, { backgroundColor: t.fill }]}>
        <Icon name={icon} size={30} color={t.textSecondary} weight="regular" />
      </View>
      <Text style={[type.title3, { color: t.text, marginTop: spacing(4), textAlign: 'center' }]}>{title}</Text>
      {body ? <Text style={[type.subhead, { color: t.textSecondary, textAlign: 'center', marginTop: spacing(1) }]}>{body}</Text> : null}
      {actionLabel && onAction ? (
        <Button title={actionLabel} onPress={onAction} size="md" variant="tinted" style={{ marginTop: spacing(4), alignSelf: 'center', paddingHorizontal: spacing(5) }} />
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
    <View style={styles.badge} accessibilityLabel={`Connection: ${label}`}>
      {status === 'connecting' || status === 'reconnecting'
        ? <ActivityIndicator size={10} color={color} />
        : <View style={[styles.badgeDot, { backgroundColor: color }]} />}
      <Text style={[type.footnote, { color: t.textSecondary, fontWeight: '500' }]}>{label}</Text>
    </View>
  );
}

/** Inline, non-blocking notice (info/warning/error) that sits in content flow. */
export function Notice({ kind = 'info', title, body, action }: { kind?: 'info' | 'warning' | 'error'; title: string; body?: string; action?: React.ReactNode }) {
  const t = useTheme();
  const color = kind === 'error' ? t.danger : kind === 'warning' ? t.warning : t.link;
  const icon = kind === 'error' ? 'exclamationmark.octagon.fill' : kind === 'warning' ? 'exclamationmark.triangle.fill' : 'info.circle.fill';
  return (
    <View style={[styles.notice, { backgroundColor: alpha(color, 0.12) }]}>
      <Icon name={icon} size={17} color={color} />
      <View style={{ flex: 1, minWidth: 0, gap: 2 }}>
        <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{title}</Text>
        {body ? <Text style={[type.footnote, { color: t.textSecondary }]}>{body}</Text> : null}
        {action}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  button: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: spacing(2), borderRadius: radii.pill },
  buttonLg: { minHeight: 50, paddingHorizontal: spacing(6) },
  buttonMd: { minHeight: 40, paddingHorizontal: spacing(4) },
  buttonSm: { minHeight: 30, paddingHorizontal: spacing(3) },
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(1.5),
    paddingHorizontal: spacing(3.5), minHeight: 36, borderRadius: radii.pill,
  },
  segmented: { flexDirection: 'row', padding: 2, borderRadius: radii.sm + 1, borderCurve: 'continuous' },
  segment: { flex: 1, flexDirection: 'row', gap: spacing(1), alignItems: 'center', justifyContent: 'center', minHeight: 30, paddingHorizontal: spacing(1), borderRadius: radii.sm - 1, borderCurve: 'continuous' },
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
  section: { fontWeight: '500', marginBottom: spacing(1.75), marginTop: spacing(5), marginLeft: edge, letterSpacing: 0.2 },
  footer: { marginTop: spacing(1.75), marginHorizontal: edge },
  field: { flexDirection: 'row', alignItems: 'center', gap: spacing(3), paddingHorizontal: edge, minHeight: 46 },
  fieldInput: { flex: 1, textAlign: 'right', paddingVertical: spacing(2.5) },
  empty: { alignItems: 'center', paddingVertical: spacing(12), paddingHorizontal: spacing(8) },
  emptyGlyph: { width: 72, height: 72, borderRadius: 22, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  badge: { flexDirection: 'row', alignItems: 'center', gap: spacing(1.5) },
  badgeDot: { width: 7, height: 7, borderRadius: 4 },
  notice: { flexDirection: 'row', gap: spacing(2.5), padding: spacing(3), borderRadius: radii.md, borderCurve: 'continuous', alignItems: 'flex-start' },
});
