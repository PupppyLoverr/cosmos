// Onboarding — the first-run welcome. Four screens over the desktop's aurora
// backdrop, with spring transitions, a progress rail, and a skip on every
// screen. The last screen hands off to the existing pairing flow.
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
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { LinearGradient } from 'expo-linear-gradient';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withSpring, withTiming } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { router } from 'expo-router';
import { spacing, type, backdropPresets } from './theme';
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

function AuroraBackdrop() {
  return (
    <View style={styles.backdrop} pointerEvents="none">
      <LinearGradient colors={AURORA_DARK as [string, string, ...string[]]} start={{ x: 0.1, y: 0 }} end={{ x: 0.9, y: 1 }} style={styles.fill} />
      <LinearGradient
        colors={[AURORA_DARK[1]!, AURORA_DARK[2]!, AURORA_DARK[0]!]}
        start={{ x: 1, y: 0.2 }} end={{ x: 0, y: 0.9 }}
        style={styles.fill}
      />
      <View style={[styles.fill, { backgroundColor: 'rgba(6,6,6,0.0)' }]} />
    </View>
  );
}

function ProgressRail({ step, total }: { step: number; total: number }) {
  return (
    <View style={styles.rail} accessibilityRole="adjustable" accessibilityLabel={`Step ${step + 1} of ${total}`}>
      {Array.from({ length: total }).map((_, i) => {
        const on = i <= step;
        return <View key={i} style={[styles.railDot, { backgroundColor: on ? '#ffffff' : 'rgba(255,255,255,0.22)' }]} />;
      })}
    </View>
  );
}

function PrimaryButton({ title, onPress, loading }: { title: string; onPress: () => void; loading?: boolean }) {
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      accessibilityRole="button"
      accessibilityLabel={title}
      disabled={loading}
      style={({ pressed }) => [styles.primary, { opacity: pressed || loading ? 0.6 : 1 }]}
    >
      <Text style={[type.headline, { color: '#060606', fontWeight: '600' }]}>{loading ? 'One moment…' : title}</Text>
    </Pressable>
  );
}

function GhostButton({ title, onPress }: { title: string; onPress: () => void }) {
  return (
    <Pressable
      onPress={() => { void Haptics.selectionAsync(); onPress(); }}
      accessibilityRole="button"
      accessibilityLabel={title}
      style={({ pressed }) => [styles.ghost, { opacity: pressed ? 0.6 : 1 }]}
    >
      <Text style={[type.body, { color: '#ffffff', fontWeight: '500' }]}>{title}</Text>
    </Pressable>
  );
}

// --- Screen 1: Welcome -----------------------------------------------------

function WelcomeScreen({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  const reduce = useReduceMotion();
  const mark = useSharedValue(0);
  const title = useSharedValue(0);
  const body = useSharedValue(0);
  useEffect(() => {
    if (reduce) { mark.set(1); title.set(1); body.set(1); return; }
    mark.set(withSpring(1, { duration: 600, dampingRatio: 0.8 }));
    title.set(withTiming(1, { duration: 420, easing: Easing.out(Easing.cubic) }));
    body.set(withTiming(1, { duration: 420, easing: Easing.out(Easing.cubic) }));
  }, [reduce, mark, title, body]);
  const markStyle = useAnimatedStyle(() => ({ opacity: mark.get(), transform: [{ scale: 0.85 + 0.15 * mark.get() }] }));
  const titleStyle = useAnimatedStyle(() => ({ opacity: title.get(), transform: [{ translateY: (1 - title.get()) * 12 }] }));
  const bodyStyle = useAnimatedStyle(() => ({ opacity: body.get(), transform: [{ translateY: (1 - body.get()) * 10 }] }));
  return (
    <View style={styles.screen}>
      <View style={styles.welcomeHero}>
        <Animated.View style={[styles.mark, markStyle]}>
          <HarnessMark size={72} />
        </Animated.View>
        <Animated.Text style={[type.largeTitle, styles.welcomeTitle, { color: '#ffffff' }, titleStyle]}>Welcome to Cosmos</Animated.Text>
        <Animated.Text style={[type.body, styles.welcomeBody, { color: 'rgba(255,255,255,0.78)' }, bodyStyle]}>
          The remote for your desktop agents. Pair a computer once, then steer it from here.
        </Animated.Text>
      </View>
      <View style={styles.welcomeCta}>
        <PrimaryButton title="Continue" onPress={onNext} />
        <GhostButton title="Skip the tour" onPress={onSkip} />
      </View>
    </View>
  );
}

export { WelcomeScreen };

// --- Screen 2: Perks --------------------------------------------------------

function PerksScreen({ onNext, onSkip }: { onNext: () => void; onSkip: () => void }) {
  return (
    <View style={styles.screen}>
      <View style={styles.perksHero}>
        <Text style={[type.footnote, { color: 'rgba(255,255,255,0.6)', fontWeight: '600', letterSpacing: 0.4 }]}>{'WHAT COSMOS DOES'}</Text>
        <Text style={[type.title2, styles.perksHeadline, { color: '#ffffff' }]}>Your desktop, in your pocket.</Text>
      </View>
      <View style={styles.perksList}>
        {PERKS.map((p, i) => <PerkRow key={p.title} {...p} index={i} />)}
      </View>
      <View style={styles.perksCta}>
        <PrimaryButton title="Continue" onPress={onNext} />
        <GhostButton title="Skip" onPress={onSkip} />
      </View>
    </View>
  );
}

function PerkRow({ icon, title, body, index }: { icon: string; title: string; body: string; index: number }) {
  const reduce = useReduceMotion();
  const v = useSharedValue(0);
  useEffect(() => {
    if (reduce) { v.set(1); return; }
    v.set(withTiming(1, { duration: 360, easing: Easing.out(Easing.cubic) }));
  }, [reduce, v]);
  const style = useAnimatedStyle(() => ({ opacity: v.get(), transform: [{ translateY: (1 - v.get()) * 16 }] }));
  return (
    <Animated.View style={[styles.perk, style]}>
      <View style={[styles.perkIcon, { backgroundColor: 'rgba(255,255,255,0.08)' }]}>
        <Icon name={icon} size={20} color="#ffffff" weight="medium" />
      </View>
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={[type.headline, { color: '#ffffff', fontWeight: '600' }]}>{title}</Text>
        <Text style={[type.footnote, { color: 'rgba(255,255,255,0.7)' }]}>{body}</Text>
      </View>
    </Animated.View>
  );
}

// --- Screen 3: Pick desktop -------------------------------------------------

function PickDesktopScreen({ onNext, onSkip, onPick }: { onNext: () => void; onSkip: () => void; onPick: (kind: DesktopKind) => void }) {
  const [picked, setPicked] = useState<DesktopKind | null>(null);
  return (
    <View style={styles.screen}>
      <View style={styles.perksHero}>
        <Text style={[type.footnote, { color: 'rgba(255,255,255,0.6)', fontWeight: '600', letterSpacing: 0.4 }]}>{'FIRST, WHICH DESKTOP'}</Text>
        <Text style={[type.title2, styles.perksHeadline, { color: '#ffffff' }]}>We&apos;ll show you the right steps.</Text>
      </View>
      <View style={styles.pickGrid}>
        {DESKTOPS.map((d) => {
          const on = picked === d.id;
          return (
            <Pressable
              key={d.id}
              onPress={() => { void Haptics.selectionAsync(); setPicked(d.id); onPick(d.id); }}
              accessibilityRole="button"
              accessibilityState={{ selected: on }}
              style={({ pressed }) => [
                styles.pickCard,
                {
                  backgroundColor: on ? 'rgba(255,255,255,0.10)' : 'rgba(255,255,255,0.04)',
                  borderColor: on ? '#ffffff' : 'rgba(255,255,255,0.12)',
                  opacity: pressed ? 0.7 : 1,
                },
              ]}
            >
              <View style={[styles.pickIcon, { backgroundColor: 'rgba(255,255,255,0.08)' }]}>
                <Icon name={d.icon} size={22} color="#ffffff" weight="medium" />
              </View>
              <Text style={[type.headline, { color: '#ffffff', fontWeight: '600' }]}>{d.label}</Text>
              {on ? (
                <View style={[styles.pickCheck, { backgroundColor: '#ffffff' }]}>
                  <Icon name="checkmark" size={11} color="#060606" weight="bold" />
                </View>
              ) : null}
            </Pressable>
          );
        })}
      </View>
      <View style={styles.perksCta}>
        <PrimaryButton title={picked ? 'Continue' : 'Pick one to continue'} onPress={onNext} />
        <GhostButton title="Skip" onPress={onSkip} />
      </View>
    </View>
  );
}

// --- Screen 4: Connect -----------------------------------------------------

function ConnectScreen({ desktopKind, onPair, onSignIn }: { desktopKind: DesktopKind | null; onPair: () => void; onSignIn: () => void }) {
  const hint = DESKTOPS.find((d) => d.id === desktopKind)?.hint ?? DESKTOPS[0]!.hint;
  return (
    <View style={styles.screen}>
      <View style={styles.perksHero}>
        <Text style={[type.footnote, { color: 'rgba(255,255,255,0.6)', fontWeight: '600', letterSpacing: 0.4 }]}>{'PAIR YOUR DESKTOP'}</Text>
        <Text style={[type.title2, styles.perksHeadline, { color: '#ffffff' }]}>One phone, one desktop, one tap.</Text>
        <Text style={[type.body, { color: 'rgba(255,255,255,0.7)', marginTop: spacing(2) }]}>{hint}</Text>
      </View>
      <View style={styles.connectCta}>
        <PrimaryButton title="Pair this desktop" onPress={onPair} />
        <GhostButton title="Sign in with Woozlit instead" onPress={onSignIn} />
      </View>
    </View>
  );
}

// --- Shell ------------------------------------------------------------------

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
      <View style={styles.stage}>
        {step === 0 && <WelcomeScreen onNext={next} onSkip={skip} />}
        {step === 1 && <PerksScreen onNext={next} onSkip={skip} />}
        {step === 2 && <PickDesktopScreen onNext={next} onSkip={skip} onPick={setKind} />}
        {step === 3 && <ConnectScreen desktopKind={kind} onPair={goPair} onSignIn={goSignIn} />}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { flex: 1 },
  backdrop: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  fill: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  topBar: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: spacing(5), paddingVertical: spacing(2) },
  rail: { flexDirection: 'row', gap: 6 },
  railDot: { width: 6, height: 6, borderRadius: 3 },
  stage: { flex: 1, paddingHorizontal: spacing(5) },
  screen: { flex: 1, justifyContent: 'space-between', paddingVertical: spacing(4) },
  welcomeHero: { flex: 1, justifyContent: 'center', alignItems: 'center', gap: spacing(3) },
  mark: { width: 88, height: 88, borderRadius: 26, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center', marginBottom: spacing(2) },
  welcomeTitle: { textAlign: 'center' },
  welcomeBody: { textAlign: 'center', maxWidth: 320 },
  welcomeCta: { gap: spacing(2) },
  perksHero: { paddingTop: spacing(4), paddingBottom: spacing(4), gap: spacing(1.5) },
  perksHeadline: { marginTop: spacing(1) },
  perksList: { flex: 1, gap: spacing(4), paddingBottom: spacing(4) },
  perk: { flexDirection: 'row', alignItems: 'center', gap: spacing(3) },
  perkIcon: { width: 44, height: 44, borderRadius: 14, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  perksCta: { gap: spacing(2) },
  pickGrid: { flex: 1, flexDirection: 'row', flexWrap: 'wrap', gap: spacing(3), paddingBottom: spacing(4) },
  pickCard: {
    width: '47%',
    flexGrow: 0,
    minHeight: 120,
    padding: spacing(4),
    gap: spacing(2.5),
    borderRadius: 18,
    borderCurve: 'continuous',
    borderWidth: StyleSheet.hairlineWidth,
    alignItems: 'flex-start',
  },
  pickIcon: { width: 44, height: 44, borderRadius: 14, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  pickCheck: { position: 'absolute', top: spacing(2.5), right: spacing(2.5), width: 22, height: 22, borderRadius: 11, alignItems: 'center', justifyContent: 'center' },
  connectCta: { gap: spacing(2), paddingBottom: spacing(2) },
  primary: {
    minHeight: 52,
    borderRadius: 14,
    borderCurve: 'continuous',
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#ffffff',
  },
  ghost: { minHeight: 44, alignItems: 'center', justifyContent: 'center' },
});
