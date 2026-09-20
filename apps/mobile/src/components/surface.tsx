import React from 'react';
import { Platform, StyleSheet, View, type StyleProp, type ViewProps, type ViewStyle } from 'react-native';
import { BlurView } from 'expo-blur';
import { GlassView } from 'expo-glass-effect';
import { Image } from 'expo-image';
import { LinearGradient } from 'expo-linear-gradient';
import { alpha, backdropPresets, radii, useTheme } from '../lib/theme';

interface GlassProps extends ViewProps {
  /** Corner radius; defaults to the large card radius. */
  radius?: number;
  /** Use for pressable controls so Liquid Glass gets its press bulge. */
  interactive?: boolean;
  /** Prefer a clearer, lighter material (chips, badges). */
  thin?: boolean;
  /** Tint the material toward a color (e.g. the accent). */
  tint?: string;
  style?: StyleProp<ViewStyle>;
}

/**
 * Card material. Resolves — in order — to native Liquid Glass (iOS 26+),
 * a system blur (older iOS) or a translucent/opaque themed fill (Android,
 * Reduce Transparency, opaque preference). Never clips a GlassView so the
 * rim highlight and press bulge survive; the blur fallback must clip.
 */
export function Glass({ radius = radii.lg, interactive, thin, tint, style, children, ...rest }: GlassProps) {
  const t = useTheme();
  const shape: ViewStyle = { borderRadius: radius, borderCurve: 'continuous' };

  if (t.liquid) {
    return (
      <GlassView
        glassEffectStyle={thin ? 'clear' : 'regular'}
        colorScheme={t.scheme}
        isInteractive={interactive}
        tintColor={tint ?? (thin ? undefined : alpha(t.surface, t.scheme === 'dark' ? 0.35 : 0.25))}
        style={[shape, style]}
        {...rest}
      >
        {children}
      </GlassView>
    );
  }

  if (t.translucent && Platform.OS === 'ios') {
    return (
      <BlurView
        tint={t.scheme === 'dark' ? (thin ? 'systemUltraThinMaterialDark' : 'systemThinMaterialDark') : (thin ? 'systemUltraThinMaterialLight' : 'systemThinMaterialLight')}
        intensity={thin ? 45 : 70}
        style={[shape, styles.clip, { backgroundColor: tint ?? alpha(t.surface, thin ? 0.25 : 0.45), borderWidth: StyleSheet.hairlineWidth, borderColor: t.glassStroke }, style]}
        {...rest}
      >
        {children}
      </BlurView>
    );
  }

  if (t.translucent) {
    // Android: a tonal, slightly translucent fill with a hairline rim reads as
    // Material "surface container" over the backdrop without a costly blur.
    return (
      <View
        style={[shape, styles.clip, { backgroundColor: tint ?? (thin ? alpha(t.surface, 0.55) : t.glassFallback), borderWidth: StyleSheet.hairlineWidth, borderColor: t.glassStroke }, style]}
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
 * Full-bleed background painted behind a screen: the theme background plus an
 * optional gradient preset or user photo, dimmed toward the theme background
 * so text and cards keep their contrast.
 */
export function Backdrop() {
  const t = useTheme();
  if (!t.hasBackdrop) return null;
  const dim = alpha(t.background, t.backdropDim);
  const veil = alpha(t.background, 0.0);

  if (t.backdrop.kind === 'photo') {
    return (
      <View style={styles.fill} pointerEvents="none">
        <Image source={{ uri: t.backdrop.uri }} style={styles.fill} contentFit="cover" cachePolicy="disk" transition={200} />
        <View style={[styles.fill, { backgroundColor: dim }]} />
        <LinearGradient colors={[veil, alpha(t.background, 0.35)]} locations={[0.55, 1]} style={styles.fill} />
      </View>
    );
  }

  if (t.backdrop.kind === 'preset') {
    const id = t.backdrop.id;
    const preset = backdropPresets.find((p) => p.id === id) ?? backdropPresets[0]!;
    const stops = preset[t.scheme];
    return (
      <View style={styles.fill} pointerEvents="none">
        <LinearGradient colors={stops as [string, string, ...string[]]} start={{ x: 0.1, y: 0 }} end={{ x: 0.9, y: 1 }} style={styles.fill} />
        <LinearGradient
          colors={[alpha(stops[1]!, 0.0), alpha(stops[2]!, 0.55), alpha(stops[0]!, 0.0)]}
          start={{ x: 1, y: 0.2 }} end={{ x: 0, y: 0.9 }}
          style={styles.fill}
        />
        <View style={[styles.fill, { backgroundColor: dim }]} />
      </View>
    );
  }
  return null;
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
