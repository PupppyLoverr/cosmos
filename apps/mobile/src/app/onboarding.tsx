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
import { Button } from '../components/bits';

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
              <Text style={[type.body, styles.wordmarkSubtitle, { color: t.textSecondary }]}>
                Run and supervise your desktop agents from your phone.
              </Text>
            </View>
            <Button title="Continue" onPress={next} style={styles.primaryButton} />
          </>
        ) : step === 1 ? (
          <>
            <View style={styles.body}>
              <Text accessibilityRole="header" style={[styles.beatHeading, { color: t.text }]}>Sessions</Text>
              <Text style={[type.body, { color: t.textSecondary, marginTop: spacing(2) }]}>Keep every desktop agent run in one list.</Text>
            </View>
            <Button title="Continue" onPress={next} style={styles.primaryButton} />
          </>
        ) : step === 2 ? (
          <>
            <View style={styles.body}>
              <Text accessibilityRole="header" style={[styles.beatHeading, { color: t.text }]}>Pair a desktop.</Text>
              <Text style={[type.body, { color: t.textSecondary, marginTop: spacing(2) }]}>
                Open Cosmos on the desktop, then scan its code.
              </Text>
            </View>
            <View style={styles.actions}>
              <Button title="Pair this phone" onPress={() => pair()} style={styles.primaryButton} />
              <Button title="Use on this Wi-Fi" variant="ghost" onPress={() => pair('local')} />
              <Button title="Later" variant="ghost" size="md" onPress={next} />
            </View>
          </>
        ) : (
          <>
            <View style={styles.body}>
              <Text accessibilityRole="header" style={[styles.beatHeading, { color: t.text }]}>You’re set.</Text>
              <Text style={[type.body, { color: t.textSecondary, marginTop: spacing(2) }]}>
                Pair from Desktops whenever you’re ready.
              </Text>
            </View>
            <Button title="Done" onPress={finish} style={styles.primaryButton} />
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
  wordmarkSubtitle: { marginTop: spacing(2), textAlign: 'center' },
  body: { flex: 1, justifyContent: 'center' },
  beatHeading: { fontSize: 22, fontWeight: '600', lineHeight: 28 },
  actions: { gap: spacing(1) },
  primaryButton: { minHeight: 48, borderRadius: 12 },
});
