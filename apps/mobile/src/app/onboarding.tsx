// First-run welcome — four quiet beats, shown once:
//   1. Wordmark.  2. What it is.  3. Pair or use on this Wi-Fi.  4. Done.
// Beat 3 hands off to the existing pair sheet (Woozlit → QR/code → grant →
// connect) or its same-network link; it never invents another auth path.
// The seen flag lives in onboarding-store next to the old first-run key.

import React, { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import Animated, { FadeIn, FadeInRight, FadeOutLeft } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';
import { motion, spacing, type, useTheme } from '../lib/theme';
import { useReduceMotion } from '../lib/motion';
import { markOnboardingSeen } from '../lib/onboarding-store';
import { Button, Icon } from '../components/bits';

const ROWS = [
  { icon: 'bubble.left.and.bubble.right', title: 'Sessions', body: 'Every agent run on your desktops, in one list.' },
  { icon: 'display', title: 'Live desktop', body: 'Watch what the agent is doing. Stop it any time.' },
  { icon: 'qrcode.viewfinder', title: 'Pair', body: 'Scan once. The desktop remembers this phone.' },
] as const;

export default function OnboardingScreen() {
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const reduce = useReduceMotion();
  const [step, setStep] = useState(0);

  const next = () => { void Haptics.selectionAsync(); setStep((s) => Math.min(3, s + 1)); };
  const finish = () => {
    void markOnboardingSeen();
    if (router.canGoBack()) router.back(); else router.replace('/');
  };
  // Leaving to pair counts as done — the flag is set before the handoff.
  const pair = (mode?: 'local') => {
    void markOnboardingSeen();
    router.replace(mode ? { pathname: '/pair', params: { mode } } : '/pair');
  };

  const enter = reduce ? undefined : FadeInRight.duration(motion.axis);
  const exit = reduce ? undefined : FadeOutLeft.duration(motion.axis * 0.75);

  return (
    <View style={[styles.shell, { backgroundColor: t.background, paddingTop: insets.top + spacing(3), paddingBottom: insets.bottom + spacing(4) }]}>
      <View style={styles.top}>
        <View style={styles.dots} accessibilityLabel={`Step ${step + 1} of 4`}>
          {[0, 1, 2, 3].map((i) => (
            <View key={i} style={[styles.dot, { backgroundColor: i === step ? t.text : t.separator }]} />
          ))}
        </View>
        {step < 3 ? (
          <Pressable onPress={finish} hitSlop={12} accessibilityRole="button" accessibilityLabel="Skip">
            <Text style={[type.body, { color: t.textSecondary }]}>Skip</Text>
          </Pressable>
        ) : null}
      </View>

      <Animated.View key={step} entering={enter} exiting={exit} style={styles.stage}>
        {step === 0 ? (
          <>
            <View style={styles.center}>
              <Animated.Text
                entering={reduce ? undefined : FadeIn.duration(motion.slow)}
                accessibilityRole="header"
                style={[styles.wordmark, { color: t.text }]}
              >
                Cosmos
              </Animated.Text>
            </View>
            <Button title="Continue" onPress={next} />
          </>
        ) : step === 1 ? (
          <>
            <View style={styles.body}>
              <Text accessibilityRole="header" style={[type.title, { color: t.text }]}>Direct your agents from anywhere.</Text>
              <View style={styles.rows}>
                {ROWS.map((r) => (
                  <View key={r.title} style={styles.row}>
                    <Icon name={r.icon} size={20} color={t.textSecondary} />
                    <View style={{ flex: 1, minWidth: 0 }}>
                      <Text style={[type.headline, { color: t.text }]}>{r.title}</Text>
                      <Text style={[type.footnote, { color: t.textSecondary }]}>{r.body}</Text>
                    </View>
                  </View>
                ))}
              </View>
            </View>
            <Button title="Continue" onPress={next} />
          </>
        ) : step === 2 ? (
          <>
            <View style={styles.body}>
              <Text accessibilityRole="header" style={[type.title, { color: t.text }]}>Pair a desktop.</Text>
              <Text style={[type.body, { color: t.textSecondary, marginTop: spacing(2) }]}>
                Open Cosmos on the desktop, then scan its code.
              </Text>
            </View>
            <View style={styles.actions}>
              <Button title="Pair this phone" onPress={() => pair()} />
              <Button title="Use on this Wi-Fi" variant="ghost" onPress={() => pair('local')} />
              <Button title="Later" variant="ghost" size="md" onPress={next} />
            </View>
          </>
        ) : (
          <>
            <View style={styles.body}>
              <Text accessibilityRole="header" style={[type.title, { color: t.text }]}>You’re set.</Text>
              <Text style={[type.body, { color: t.textSecondary, marginTop: spacing(2) }]}>
                Pair from Desktops whenever you’re ready.
              </Text>
            </View>
            <Button title="Done" onPress={finish} />
          </>
        )}
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { flex: 1 },
  top: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: spacing(5), minHeight: 44 },
  dots: { flexDirection: 'row', gap: 6 },
  dot: { width: 6, height: 6, borderRadius: 3 },
  stage: { flex: 1, paddingHorizontal: spacing(5), paddingTop: spacing(6), justifyContent: 'space-between' },
  center: { flex: 1, alignItems: 'center', justifyContent: 'center' },
  wordmark: { fontSize: 44, fontWeight: '600', letterSpacing: -0.8 },
  body: { flex: 1, paddingTop: spacing(8) },
  rows: { marginTop: spacing(8), gap: spacing(5) },
  row: {
    flexDirection: 'row', alignItems: 'flex-start', gap: spacing(4),
  },
  actions: { gap: spacing(1) },
});
