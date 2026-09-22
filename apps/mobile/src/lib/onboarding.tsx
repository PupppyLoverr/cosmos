// Onboarding — the first-run welcome. Four screens over the desktop's aurora
// backdrop with crossfading stage transitions, drifting glow orbs, a
// progress rail, and a skip on every screen. The last screen hands off to the
// existing pairing flow.
//
// Design references:
// - crates/ui/src/shell.rs `render_first_run_welcome` — the desktop's
//   "Welcome to Cosmos" card. Same copy tone, same aurora backdrop.
// - apps/mobile/src/lib/theme.ts `backdropPresets.aurora` — the desktop's
//   signature new-thread gradient.
// - Best practice (2026): 3-5 screens, single value prop per screen,
//   skip option, progress indicator, reach the first meaningful action
//   within 60 seconds.

import React, { useEffect, useState } from 'react';
import { Pressable, StyleSheet, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import { LinearGradient } from 'expo-linear-gradient';
import Animated, {
  Easing,
  runOnJS,
  useAnimatedStyle,
  useSharedValue,
  withDelay,
  withRepeat,
  withSpring,
  withTiming,
  type SharedValue,
} from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { router } from 'expo-router';
import { enterBezier, spacing, type, backdropPresets } from './theme';
import { useReduceMotion } from './motion';
import { Icon } from '../components/bits';
import { HarnessMark } from '../components/harness-mark';

export type DesktopKind = 'mac' | 'windows' | 'linux' | 'other';

const AURORA_DARK = backdropPresets.find((p) => p.id === 'aurora')!.dark;

const PERKS = [
  { icon: 'bubble.left.and.bubble.right.fill', title: 'Talk to every desktop', body: 'Your Mac, Windows, and Linux boxes show up in one list. Pick one and start.' },
  { icon: 'lock.shield.fill', title: 'You approve each phone', body: 'A desktop only takes control from a phone it has trusted. Pair once, then it remembers you.' },
  { icon: 'display', title: 'Watch and stop instantly', body: 'See what Computer Use is doing on screen and interrupt it the moment it goes wrong.' },
  { icon: 'gearshape.fill', title: 'Your agent, your rules', body: 'Pick the agent, the model, the effort, and the sandbox. Cosmos runs the turn your way.' },
] as const;

const DESKTOPS: { id: DesktopKind; label: string; icon: string; hint: string }[] = [
  { id: 'mac', label: 'Mac', icon: 'laptopcomputer', hint: 'On the Mac: Cosmos → Settings → Devices → Pair phone.' },
  { id: 'windows', label: 'Windows', icon: 'pc', hint: 'On Windows: Cosmos → Settings → Devices → Pair phone.' },
  { id: 'linux', label: 'Linux', icon: 'server.rack', hint: 'On Linux: Cosmos → Settings → Devices → Pair phone.' },
  { id: 'other', label: 'Other', icon: 'desktopcomputer', hint: 'Pair any desktop running Cosmos over the same network or your account.' },
];

const stageEasing = Easing.bezier(enterBezier[0], enterBezier[1], enterBezier[2], enterBezier[3]);

// --- Shared bits ------------------------------------------------------------

/** Staggered entrance: fade + rise, `delay` ms after mount. */
function Enter({ delay = 0, children, style }: { delay?: number; children: React.ReactNode; style?: object }) {
  const reduce = useReduceMotion();
  const v = useSharedValue(reduce ? 1 : 0);
  useEffect(() => {
    if (reduce) { v.set(1); return; }
    v.set(withDelay(delay, withTiming(1, { duration: 420, easing: stageEasing })));
  }, [delay, reduce, v]);
  const anim = useAnimatedStyle(() => ({ opacity: v.get(), transform: [{ translateY: (1 - v.get()) * 14 }] }));
  return <Animated.View style={[anim, style]}>{children}</Animated.View>;
}

/** A soft colored bloom — a small tinted disc with a huge blurred shadow,
 *  drifting slowly across the aurora. */
function GlowOrb({ color, size, x, y, drift, duration }: { color: string; size: number; x: number; y: number; drift: number; duration: number }) {
  const reduce = useReduceMotion();
  const p = useSharedValue(0);
  useEffect(() => {
    if (reduce) return;
    p.set(withRepeat(withTiming(1, { duration, easing: Easing.inOut(Easing.sin) }), -1, true));
  }, [duration, p, reduce]);
  const style = useAnimatedStyle(() => ({
    transform: [
      { translateX: drift * Math.sin(p.get() * Math.PI) },
      { translateY: drift * 0.7 * Math.cos(p.get() * Math.PI) },
      { scale: 1 + 0.18 * Math.sin(p.get() * Math.PI * 2) },
    ],
  }));
  return (
    <Animated.View
      style={[{
        position: 'absolute', left: x, top: y,
        width: size, height: size, borderRadius: size / 2,
        backgroundColor: color,
        shadowColor: color, shadowOpacity: 1, shadowRadius: size * 1.4, shadowOffset: { width: 0, height: 0 },
        opacity: 0.55,
      }, style]}
    />
  );
}

function AuroraBackdrop() {
  return (
    <View style={styles.backdrop} pointerEvents="none">
      <LinearGradient colors={AURORA_DARK as [string, string, ...string[]]} start={{ x: 0.1, y: 0 }} end={{ x: 0.9, y: 1 }} style={styles.fill} />
      <LinearGradient
        colors={[AURORA_DARK[1]!, AURORA_DARK[2]!, AURORA_DARK[0]!]}
        start={{ x: 1, y: 0.2 }} end={{ x: 0, y: 0.9 }}
        style={styles.fill}
      />
      <GlowOrb color={AURORA_DARK[2]!} size={52} x={-10} y={120} drift={34} duration={9000} />
      <GlowOrb color={AURORA_DARK[0]!} size={44} x={290} y={460} drift={-28} duration={11000} />
      {/* Bottom vignette keeps the CTA legible over the bloom. */}
      <LinearGradient colors={['rgba(0,0,0,0)', 'rgba(0,0,0,0.34)']} start={{ x: 0.5, y: 0 }} end={{ x: 0.5, y: 1 }} style={styles.fill} />
    </View>
  );
}

/** Dots where the active step is an elongated pill. */
function RailDot({ on, done }: { on: boolean; done: boolean }) {
  const reduce = useReduceMotion();
  const v = useSharedValue(on ? 1 : 0);
  useEffect(() => {
    v.set(reduce ? (on ? 1 : 0) : withTiming(on ? 1 : 0, { duration: 260, easing: stageEasing }));
  }, [on, reduce, v]);
  const style = useAnimatedStyle(() => ({
    width: 6 + v.get() * 14,
    opacity: done ? 0.55 : 0.35 + v.get() * 0.65,
  }));
  return <Animated.View style={[styles.railDot, style]} />;
}

function ProgressRail({ step, total }: { step: number; total: number }) {
  return (
    <View style={styles.rail} accessibilityRole="adjustable" accessibilityLabel={`Step ${step + 1} of ${total}`}>
      {Array.from({ length: total }).map((_, i) => <RailDot key={i} on={i === step} done={i < step} />)}
    </View>
  );
}

/** Pressable with a spring scale on press. `style` sits on the inner view so
 *  the scale transform wraps it. */
function SpringPress({ onPress, scale = 0.96, style, children, disabled, accessibilityLabel, accessibilityHint, accessibilityState }: {
  onPress: () => void;
  scale?: number;
  style?: StyleProp<ViewStyle>;
  children: React.ReactNode;
  disabled?: boolean;
  accessibilityLabel?: string;
  accessibilityHint?: string;
  accessibilityState?: { selected?: boolean };
}) {
  const reduce = useReduceMotion();
  const v = useSharedValue(1);
  const anim = useAnimatedStyle(() => ({ transform: [{ scale: v.get() }] }));
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      onPressIn={() => { if (!reduce) v.set(withSpring(scale, { duration: 160, dampingRatio: 0.7 })); }}
      onPressOut={() => { if (!reduce) v.set(withSpring(1, { duration: 220, dampingRatio: 0.7 })); }}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel}
      accessibilityHint={accessibilityHint}
      accessibilityState={accessibilityState}
    >
      <Animated.View style={[anim, style]}>{children}</Animated.View>
    </Pressable>
  );
}

function PrimaryButton({ title, onPress, loading, disabled }: { title: string; onPress: () => void; loading?: boolean; disabled?: boolean }) {
  return (
    <SpringPress onPress={onPress} disabled={loading || disabled} accessibilityLabel={title} scale={0.97}>
      <View style={[styles.primary, { opacity: disabled || loading ? 0.5 : 1 }]}>
        <Text style={[type.headline, { color: '#060606', fontWeight: '600' }]}>{loading ? 'One moment…' : title}</Text>
      </View>
    </SpringPress>
  );
}

function GhostButton({ title, onPress }: { title: string; onPress: () => void }) {
  return (
    <SpringPress onPress={onPress} accessibilityLabel={title} scale={0.96}>
      <View style={styles.ghost}>
        <Text style={[type.body, { color: '#ffffff', fontWeight: '500' }]}>{title}</Text>
      </View>
    </SpringPress>
  );
}

function ScreenKicker({ children }: { children: string }) {
  return <Text style={[type.footnote, { color: 'rgba(255,255,255,0.6)', fontWeight: '600', letterSpacing: 0.4 }]}>{children}</Text>;
}

// --- Screen 1: Welcome -----------------------------------------------------

function WelcomeScreen({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  const reduce = useReduceMotion();
  const ring = useSharedValue(0);
  useEffect(() => {
    if (reduce) { ring.set(1); return; }
    // A soft pulse on the halo ring so the mark feels alive, not static.
    ring.set(withRepeat(withTiming(1, { duration: 2600, easing: Easing.inOut(Easing.sin) }), -1, true));
  }, [reduce, ring]);
  const ringStyle = useAnimatedStyle(() => ({
    opacity: 0.28 + ring.get() * 0.3,
    transform: [{ scale: 1 + ring.get() * 0.06 }],
  }));
  return (
    <View style={styles.screen}>
      <View style={styles.welcomeHero}>
        <Enter delay={0}>
          <View style={styles.markWrap}>
            <Animated.View style={[styles.markRing, ringStyle]} />
            <View style={styles.mark}>
              <HarnessMark size={76} />
            </View>
          </View>
        </Enter>
        <Enter delay={120}>
          <Text style={[styles.welcomeTitle, { color: '#ffffff' }]}>Welcome to Cosmos</Text>
        </Enter>
        <Enter delay={220}>
          <Text style={[type.body, styles.welcomeBody, { color: 'rgba(255,255,255,0.78)' }]}>
            The remote for your desktop agents. Pair a computer once, then steer it from here.
          </Text>
        </Enter>
      </View>
      <Enter delay={320} style={styles.welcomeCta}>
        <PrimaryButton title="Continue" onPress={onNext} />
        <GhostButton title="Skip the tour" onPress={onSkip} />
      </Enter>
    </View>
  );
}

export { WelcomeScreen };

// --- Screen 2: Perks --------------------------------------------------------

function PerksScreen({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  return (
    <View style={styles.screen}>
      <View style={styles.perksHero}>
        <Enter delay={0}><ScreenKicker>{'WHAT COSMOS DOES'}</ScreenKicker></Enter>
        <Enter delay={70}>
          <Text style={[type.title2, styles.perksHeadline, { color: '#ffffff' }]}>Your desktop, in your pocket.</Text>
        </Enter>
      </View>
      <View style={styles.perksList}>
        {PERKS.map((p, i) => <PerkRow key={p.title} {...p} index={i} />)}
      </View>
      <Enter delay={140 + PERKS.length * 80} style={styles.perksCta}>
        <PrimaryButton title="Continue" onPress={onNext} />
        <GhostButton title="Skip" onPress={onSkip} />
      </Enter>
    </View>
  );
}

function PerkRow({ icon, title, body, index }: { icon: string; title: string; body: string; index: number }) {
  return (
    <Enter delay={140 + index * 80}>
      <View style={styles.perk}>
        <View style={styles.perkIcon}>
          <Icon name={icon} size={19} color="#ffffff" weight="medium" />
        </View>
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text style={[type.headline, { color: '#ffffff', fontWeight: '600' }]}>{title}</Text>
          <Text style={[type.footnote, { color: 'rgba(255,255,255,0.68)', marginTop: 2 }]}>{body}</Text>
        </View>
      </View>
    </Enter>
  );
}

// --- Screen 3: Pick desktop -------------------------------------------------

function PickDesktopScreen({ onNext, onSkip, onPick }: { onNext: () => void; onSkip: () => void; onPick: (kind: DesktopKind) => void }) {
  const [picked, setPicked] = useState<DesktopKind | null>(null);
  return (
    <View style={styles.screen}>
      <View style={styles.perksHero}>
        <Enter delay={0}><ScreenKicker>{'FIRST, WHICH DESKTOP'}</ScreenKicker></Enter>
        <Enter delay={70}>
          <Text style={[type.title2, styles.perksHeadline, { color: '#ffffff' }]}>We&apos;ll show you the right steps.</Text>
        </Enter>
      </View>
      <View style={styles.pickGrid}>
        {DESKTOPS.map((d, i) => <PickCard key={d.id} d={d} index={i} on={picked === d.id} onPick={() => { setPicked(d.id); onPick(d.id); }} />)}
      </View>
      <Enter delay={160 + DESKTOPS.length * 60} style={styles.perksCta}>
        <PrimaryButton title={picked ? 'Continue' : 'Pick one to continue'} onPress={onNext} disabled={!picked} />
        <GhostButton title="Skip" onPress={onSkip} />
      </Enter>
    </View>
  );
}

function PickCard({ d, index, on, onPick }: { d: (typeof DESKTOPS)[number]; index: number; on: boolean; onPick: () => void }) {
  const reduce = useReduceMotion();
  const check = useSharedValue(on ? 1 : 0);
  useEffect(() => {
    check.set(reduce ? (on ? 1 : 0) : withSpring(on ? 1 : 0, { duration: 300, dampingRatio: 0.65 }));
  }, [on, reduce, check]);
  const checkStyle = useAnimatedStyle(() => ({
    opacity: check.get(),
    transform: [{ scale: 0.5 + check.get() * 0.5 }],
  }));
  return (
    <Enter delay={120 + index * 60} style={styles.pickCell}>
      <SpringPress onPress={onPick} accessibilityLabel={d.label} accessibilityState={{ selected: on }} scale={0.95}>
        <View style={[styles.pickCard, {
          backgroundColor: on ? 'rgba(255,255,255,0.12)' : 'rgba(255,255,255,0.045)',
          borderColor: on ? '#ffffff' : 'rgba(255,255,255,0.12)',
        }]}>
          <View style={[styles.pickIcon, { backgroundColor: on ? 'rgba(255,255,255,0.16)' : 'rgba(255,255,255,0.08)' }]}>
            <Icon name={d.icon} size={22} color="#ffffff" weight="medium" />
          </View>
          <Text style={[type.headline, { color: '#ffffff', fontWeight: '600' }]}>{d.label}</Text>
          <Animated.View style={[styles.pickCheck, { backgroundColor: '#ffffff' }, checkStyle]}>
            <Icon name="checkmark" size={11} color="#060606" weight="bold" />
          </Animated.View>
        </View>
      </SpringPress>
    </Enter>
  );
}

// --- Screen 4: Connect ------------------------------------------------------

/** Three dots flowing phone → desktop, one after another. */
function LinkDots() {
  const reduce = useReduceMotion();
  const p = useSharedValue(0);
  useEffect(() => {
    if (reduce) return;
    p.set(withRepeat(withTiming(3, { duration: 1600, easing: Easing.linear }), -1, false));
  }, [p, reduce]);
  return (
    <View style={styles.linkDots}>
      {[0, 1, 2].map((i) => <LinkDot key={i} phase={p} i={i} />)}
    </View>
  );
}

function LinkDot({ phase, i }: { phase: SharedValue<number>; i: number }) {
  const style = useAnimatedStyle(() => {
    // Each dot peaks when the phase crosses its slot, then falls back.
    const d = Math.abs(phase.get() - (i + 0.5));
    const glow = Math.max(0, 1 - d);
    return { opacity: 0.25 + glow * 0.75, transform: [{ scale: 0.8 + glow * 0.35 }] };
  });
  return <Animated.View style={[styles.linkDot, style]} />;
}

function ConnectScreen({ desktopKind, onPair, onSignIn }: { desktopKind: DesktopKind | null; onPair: () => void; onSignIn: () => void }) {
  const hint = DESKTOPS.find((d) => d.id === desktopKind)?.hint ?? DESKTOPS[0]!.hint;
  return (
    <View style={styles.screen}>
      <View style={styles.perksHero}>
        <Enter delay={0}><ScreenKicker>{'PAIR YOUR DESKTOP'}</ScreenKicker></Enter>
        <Enter delay={70}>
          <Text style={[type.title2, styles.perksHeadline, { color: '#ffffff' }]}>One phone, one desktop, one tap.</Text>
        </Enter>
      </View>
      <Enter delay={140} style={{ alignItems: 'center' }}>
        <View style={styles.linkRow}>
          <View style={styles.linkTile}>
            <Icon name="iphone" size={26} color="#ffffff" weight="medium" />
          </View>
          <LinkDots />
          <View style={[styles.linkTile, { borderColor: 'rgba(255,255,255,0.4)' }]}>
            <Icon name={DESKTOPS.find((d) => d.id === desktopKind)?.icon ?? 'desktopcomputer'} size={26} color="#ffffff" weight="medium" />
          </View>
        </View>
        <Text style={[type.subhead, styles.connectHint, { color: 'rgba(255,255,255,0.72)' }]}>{hint}</Text>
      </Enter>
      <Enter delay={260} style={styles.connectCta}>
        <PrimaryButton title="Pair this desktop" onPress={onPair} />
        <GhostButton title="Sign in with Woozlit instead" onPress={onSignIn} />
      </Enter>
    </View>
  );
}

// --- Shell ------------------------------------------------------------------

/**
 * The stage keeps the outgoing screen mounted while the incoming one slides
 * in — a 30pt push + fade, the desktop's enter curve.
 */
function Stage({ step, children }: { step: number; children: (s: number) => React.ReactNode }) {
  const reduce = useReduceMotion();
  const [leaving, setLeaving] = useState<number | null>(null);
  const [shown, setShown] = useState(step);
  if (step !== shown) {
    // Adjust during render: the outgoing screen stays mounted one beat to
    // animate out while the new one enters.
    setShown(step);
    if (!reduce) setLeaving(shown);
  }
  return (
    <View style={styles.stage}>
      {leaving !== null ? (
        <StageLayer key={leaving} mode="leave" onDone={() => setLeaving(null)}>{children(leaving)}</StageLayer>
      ) : null}
      <StageLayer key={shown} mode={leaving !== null ? 'enter' : 'idle'}>{children(shown)}</StageLayer>
    </View>
  );
}

function StageLayer({ mode, onDone, children }: { mode: 'enter' | 'leave' | 'idle'; onDone?: () => void; children: React.ReactNode }) {
  const reduce = useReduceMotion();
  const v = useSharedValue(mode === 'enter' ? 0 : 1);
  useEffect(() => {
    if (reduce) {
      if (mode === 'leave' && onDone) onDone();
      v.set(1);
      return;
    }
    if (mode === 'enter') v.set(withTiming(1, { duration: 340, easing: stageEasing }));
    if (mode === 'leave') {
      v.set(withTiming(0, { duration: 240, easing: Easing.in(Easing.cubic) }, (done) => { if (done && onDone) runOnJS(onDone)(); }));
    }
  }, [mode, reduce, v, onDone]);
  const style = useAnimatedStyle(() => ({
    opacity: v.get(),
    transform: [{ translateX: (1 - v.get()) * (mode === 'leave' ? -30 : 30) }],
  }));
  return <Animated.View style={[styles.stageLayer, style]} pointerEvents={mode === 'leave' ? 'none' : 'auto'}>{children}</Animated.View>;
}

export function Onboarding({ onFinish, onSkip }: { onFinish: (kind: DesktopKind | null) => void; onSkip: () => void }) {
  const insets = useSafeAreaInsets();
  const [step, setStep] = useState(0);
  const [kind, setKind] = useState<DesktopKind | null>(null);
  const total = 4;

  const goPair = () => {
    void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
    onFinish(kind);
  };
  const goSignIn = () => {
    onFinish(kind);
    router.push('/sign-in');
  };
  const skip = () => {
    void Haptics.selectionAsync();
    onSkip();
  };
  const next = () => {
    if (step < total - 1) { void Haptics.selectionAsync(); setStep((s) => s + 1); }
    else goPair();
  };

  return (
    <View style={[styles.shell, { paddingTop: insets.top + spacing(2), paddingBottom: insets.bottom + spacing(3) }]}>
      <AuroraBackdrop />
      <View style={styles.topBar}>
        <ProgressRail step={step} total={total} />
        <Pressable onPress={skip} hitSlop={12} accessibilityRole="button" accessibilityLabel="Skip onboarding">
          <Text style={[type.footnote, { color: 'rgba(255,255,255,0.7)', fontWeight: '600' }]}>Skip</Text>
        </Pressable>
      </View>
      <Stage step={step}>
        {(s) => {
          switch (s) {
            case 0: return <WelcomeScreen onNext={next} onSkip={skip} />;
            case 1: return <PerksScreen onNext={next} onSkip={skip} />;
            case 2: return <PickDesktopScreen onNext={next} onSkip={skip} onPick={setKind} />;
            default: return <ConnectScreen desktopKind={kind} onPair={goPair} onSignIn={goSignIn} />;
          }
        }}
      </Stage>
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { flex: 1 },
  backdrop: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  fill: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  topBar: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: spacing(5), paddingVertical: spacing(2) },
  rail: { flexDirection: 'row', gap: 6, alignItems: 'center' },
  railDot: { width: 6, height: 6, borderRadius: 3, backgroundColor: '#ffffff' },
  stage: { flex: 1, paddingHorizontal: spacing(5) },
  stageLayer: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  screen: { flex: 1, justifyContent: 'space-between', paddingVertical: spacing(4) },
  welcomeHero: { flex: 1, justifyContent: 'center', alignItems: 'center', gap: spacing(4) },
  markWrap: { alignItems: 'center', justifyContent: 'center', marginBottom: spacing(2) },
  markRing: {
    position: 'absolute', width: 128, height: 128, borderRadius: 64,
    borderWidth: 1, borderColor: 'rgba(255,255,255,0.5)',
  },
  mark: {
    width: 96, height: 96, borderRadius: 30, borderCurve: 'continuous',
    alignItems: 'center', justifyContent: 'center',
    backgroundColor: 'rgba(255,255,255,0.10)',
    borderWidth: StyleSheet.hairlineWidth, borderColor: 'rgba(255,255,255,0.28)',
    shadowColor: '#ffffff', shadowOpacity: 0.35, shadowRadius: 30, shadowOffset: { width: 0, height: 0 },
  },
  welcomeTitle: { fontSize: 34, fontWeight: '700', letterSpacing: 0.2, lineHeight: 40, textAlign: 'center' },
  welcomeBody: { textAlign: 'center', maxWidth: 320 },
  welcomeCta: { gap: spacing(2) },
  perksHero: { paddingTop: spacing(4), paddingBottom: spacing(4), gap: spacing(1.5) },
  perksHeadline: { marginTop: spacing(1) },
  perksList: { flex: 1, gap: spacing(2.5), paddingBottom: spacing(4) },
  perk: {
    flexDirection: 'row', alignItems: 'center', gap: spacing(3),
    backgroundColor: 'rgba(255,255,255,0.05)',
    borderWidth: StyleSheet.hairlineWidth, borderColor: 'rgba(255,255,255,0.1)',
    borderRadius: 18, borderCurve: 'continuous',
    padding: spacing(3.5),
  },
  perkIcon: {
    width: 42, height: 42, borderRadius: 13, borderCurve: 'continuous',
    alignItems: 'center', justifyContent: 'center',
    backgroundColor: 'rgba(255,255,255,0.1)',
    borderWidth: StyleSheet.hairlineWidth, borderColor: 'rgba(255,255,255,0.16)',
  },
  perksCta: { gap: spacing(2) },
  pickGrid: { flex: 1, flexDirection: 'row', flexWrap: 'wrap', gap: spacing(3), paddingBottom: spacing(4) },
  pickCell: { width: '47%' },
  pickCard: {
    minHeight: 124,
    padding: spacing(4),
    gap: spacing(2.5),
    borderRadius: 18,
    borderCurve: 'continuous',
    borderWidth: 1,
    alignItems: 'flex-start',
    overflow: 'hidden',
  },
  pickIcon: { width: 44, height: 44, borderRadius: 14, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  pickCheck: { position: 'absolute', top: spacing(2.5), right: spacing(2.5), width: 22, height: 22, borderRadius: 11, alignItems: 'center', justifyContent: 'center' },
  linkRow: { flexDirection: 'row', alignItems: 'center', gap: spacing(3) },
  linkTile: {
    width: 72, height: 72, borderRadius: 22, borderCurve: 'continuous',
    alignItems: 'center', justifyContent: 'center',
    backgroundColor: 'rgba(255,255,255,0.07)',
    borderWidth: StyleSheet.hairlineWidth, borderColor: 'rgba(255,255,255,0.18)',
    shadowColor: '#ffffff', shadowOpacity: 0.18, shadowRadius: 18, shadowOffset: { width: 0, height: 0 },
  },
  linkDots: { flexDirection: 'row', gap: spacing(2), alignItems: 'center' },
  linkDot: { width: 7, height: 7, borderRadius: 4, backgroundColor: '#ffffff' },
  connectHint: { textAlign: 'center', maxWidth: 300, marginTop: spacing(5) },
  connectCta: { gap: spacing(2), paddingBottom: spacing(2) },
  primary: {
    minHeight: 52,
    borderRadius: 14,
    borderCurve: 'continuous',
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#ffffff',
    shadowColor: '#000000', shadowOpacity: 0.25, shadowRadius: 14, shadowOffset: { width: 0, height: 6 },
  },
  ghost: { minHeight: 44, alignItems: 'center', justifyContent: 'center' },
});
