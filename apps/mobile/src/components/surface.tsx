import React from 'react';
import { Platform, StyleSheet, View, type StyleProp, type ViewProps, type ViewStyle } from 'react-native';
import { BlurView } from 'expo-blur';
import { GlassView } from 'expo-glass-effect';
import { LinearGradient } from 'expo-linear-gradient';
import { alpha, backdropPresets, radii, useTheme, type Material, type Theme } from '../lib/theme';
import { Wallpaper } from './wallpaper';

interface GlassProps extends ViewProps {
  /** Corner radius; defaults to the large card radius. */
  radius?: number;
  /** Use for pressable controls so Liquid Glass gets its press bulge. */
  interactive?: boolean;
  /** Prefer a clearer, lighter material (chips, badges). */
  thin?: boolean;
  /** Tint the material toward a color (e.g. the accent). */
  tint?: string;
  /** Force a material regardless of the theme (e.g. an always-solid preview). */
  material?: Material;
  style?: StyleProp<ViewStyle>;
}

function blurTint(material: Material, dark: boolean, thin: boolean | undefined) {
  if (material === 'acrylic') {
    return dark
      ? (thin ? 'systemMaterialDark' : 'systemThickMaterialDark')
      : (thin ? 'systemMaterialLight' : 'systemThickMaterialLight');
  }
  return dark
    ? (thin ? 'systemUltraThinMaterialDark' : 'systemThinMaterialDark')
    : (thin ? 'systemUltraThinMaterialLight' : 'systemThinMaterialLight');
}

/**
 * Surface material. Resolves — in order — to native Liquid Glass (iOS 26+),
 * a system blur (acrylic = thick, frosted = thin) or a tonal/opaque themed
 * fill (Android, Reduce Transparency, Solid preference). Never clips a
 * GlassView so the rim highlight and press bulge survive; the blur fallback
 * must clip.
 */
export function Glass({ radius = radii.lg, interactive, thin, tint, material, style, children, ...rest }: GlassProps) {
  const t = useTheme();
  const m = material ?? t.material;
  const dark = t.scheme === 'dark';
  const shape: ViewStyle = { borderRadius: radius, borderCurve: 'continuous' };

  if (m === 'glass' && t.liquid) {
    return (
      <GlassView
        glassEffectStyle={thin ? 'clear' : 'regular'}
        colorScheme={t.scheme}
        isInteractive={interactive}
        tintColor={tint ?? (thin ? undefined : alpha(t.surface, dark ? 0.3 : 0.2))}
        style={[shape, style]}
        {...rest}
      >
        {children}
      </GlassView>
    );
  }

  if (m !== 'opaque' && Platform.OS === 'ios') {
    const acrylic = m === 'acrylic' || m === 'glass';
    return (
      <BlurView
        tint={blurTint(acrylic ? 'acrylic' : 'frosted', dark, thin)}
        intensity={acrylic ? 85 : (thin ? 40 : 60)}
        style={[
          shape, styles.clip,
          { backgroundColor: tint ?? alpha(t.surface, acrylic ? (thin ? 0.35 : 0.55) : (thin ? 0.18 : 0.32)), borderWidth: StyleSheet.hairlineWidth, borderColor: t.glassStroke },
          style,
        ]}
        {...rest}
      >
        {children}
      </BlurView>
    );
  }

  if (m !== 'opaque') {
    // Android: tonal, slightly translucent fill with a hairline rim reads as
    // a Material "surface container" over the wallpaper without a costly blur.
    return (
      <View
        style={[shape, styles.clip, { backgroundColor: tint ?? (thin ? alpha(t.surface, 0.6) : t.glassFallback), borderWidth: StyleSheet.hairlineWidth, borderColor: t.glassStroke }, style]}
        {...rest}
      >
        {children}
      </View>
    );
  }

  return (
    <View style={[shape, styles.clip, { backgroundColor: tint ?? t.surface }, style]} {...rest}>
      {children}
    </View>
  );
}

/**
 * Content card. Solid theme surface on the plain paper/near-black field —
 * material only earns its place over a wallpaper, where it keeps text legible.
 */
export function Card({ radius = radii.lg, style, children, ...rest }: ViewProps & { radius?: number }) {
  const t = useTheme();
  if (t.hasBackdrop && t.translucent) {
    return <Glass radius={radius} style={style} {...rest}>{children}</Glass>;
  }
  return (
    <View
      style={[{ borderRadius: radius, borderCurve: 'continuous', backgroundColor: t.surface, borderWidth: 0 }, styles.clip, style]}
      {...rest}
    >
      {children}
    </View>
  );
}

/** Gradient preset painted for a theme; shared by the screen backdrop and the appearance previews. */
export function PresetGradient({ id, scheme, style }: { id: string; scheme: Theme['scheme']; style?: StyleProp<ViewStyle> }) {
  const preset = backdropPresets.find((p) => p.id === id) ?? backdropPresets[0]!;
  const stops = preset[scheme];
  return (
    <View style={[styles.fill, style]} pointerEvents="none">
      <LinearGradient colors={stops as [string, string, ...string[]]} start={{ x: 0.1, y: 0 }} end={{ x: 0.9, y: 1 }} style={styles.fill} />
      <LinearGradient
        colors={[alpha(stops[1]!, 0.0), alpha(stops[2]!, 0.55), alpha(stops[0]!, 0.0)]}
        start={{ x: 1, y: 0.2 }} end={{ x: 0, y: 0.9 }}
        style={styles.fill}
      />
    </View>
  );
}

/**
 * Full-bleed background painted behind a screen: the theme background plus an
 * optional gradient preset or the user's photo (with its image treatment),
 * dimmed toward the theme background so text and surfaces keep contrast.
 */
export function Backdrop() {
  const t = useTheme();
  const b = t.backdrop;
  if (b.kind === 'none') return null;
  const dim = alpha(t.background, t.backdropDim);

  if (b.kind === 'photo') {
    return (
      <View style={styles.fill} pointerEvents="none">
        <Wallpaper uri={b.uri} effect={t.effect} light={t.scheme === 'light'} />
        <View style={[styles.fill, { backgroundColor: dim }]} />
        <LinearGradient
          colors={[alpha(t.background, 0.25), alpha(t.background, 0), alpha(t.background, 0.45)]}
          locations={[0, 0.3, 1]}
          style={styles.fill}
        />
      </View>
    );
  }

  return (
    <View style={styles.fill} pointerEvents="none">
      <PresetGradient id={b.id} scheme={t.scheme} />
      <View style={[styles.fill, { backgroundColor: dim }]} />
    </View>
  );
}

/** Root container for every route: theme background + backdrop. */
export function Screen({ style, children, ...rest }: ViewProps) {
  const t = useTheme();
  return (
    <View style={[styles.screen, { backgroundColor: t.background }, style]} {...rest}>
      <Backdrop />
      {children}
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  fill: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  clip: { overflow: 'hidden' },
});
