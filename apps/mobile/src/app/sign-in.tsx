import React, { useEffect, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withDelay, withTiming } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useCosmos } from '../lib/client';
import { alpha, useTheme, spacing, type, radii } from '../lib/theme';
import { Button, Icon, IconButton, Notice } from '../components/bits';
import { Glass, Screen } from '../components/surface';
import { HarnessMark } from '../components/harness-mark';
import { useReduceMotion } from '../lib/motion';

const PERKS: { icon: string; title: string; body: string }[] = [
  { icon: 'desktopcomputer', title: 'Every desktop, one list', body: 'Desktops signed into your account show up here automatically.' },
  { icon: 'lock.shield', title: 'You approve each phone', body: 'A desktop only takes control from a phone it has trusted.' },
  { icon: 'display', title: 'Watch and stop from anywhere', body: 'See what Computer Use is doing and interrupt it instantly.' },
];

function Perk({ icon, title, body, index }: (typeof PERKS)[number] & { index: number }) {
  const t = useTheme();
  const reduce = useReduceMotion();
  const v = useSharedValue(0);
  useEffect(() => {
    v.set(reduce ? 1 : withDelay(120 + index * 90, withTiming(1, { duration: 420, easing: Easing.out(Easing.cubic) })));
  }, [index, reduce, v]);
  const style = useAnimatedStyle(() => ({ opacity: v.get(), transform: [{ translateY: (1 - v.get()) * 14 }] }));
  return (
    <Animated.View style={[styles.perk, style]}>
      <View style={[styles.perkIcon, { backgroundColor: t.fill }]}>
        <Icon name={icon} size={20} color={t.text} weight="medium" />
      </View>
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={[type.subhead, { color: t.text, fontWeight: '600' }]}>{title}</Text>
        <Text style={[type.footnote, { color: t.textSecondary }]}>{body}</Text>
      </View>
    </Animated.View>
  );
}

export default function SignInScreen() {
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const client = useCosmos();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Sign-in can be the root route when deep-linked — router.back() then
  // throws GO_BACK unhandled and swallows the completed session.
  const leave = () => {
    if (router.canGoBack()) router.back();
    else router.replace('/');
  };

  const signIn = async () => {
    setBusy(true);
    setError(null);
    try {
      await client.signIn();
      leave();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Screen style={[styles.screen, { paddingTop: insets.top + spacing(4), paddingBottom: insets.bottom + spacing(4) }]}>
      <View style={[styles.close, { top: insets.top + spacing(2) }]}>
        <IconButton icon="xmark" label="Close" filled={false} onPress={leave} />
      </View>
      <View style={styles.hero}>
        <View style={[styles.mark, { backgroundColor: alpha(t.accent, 0.12) }]}>
          <HarnessMark size={72} />
        </View>
        <Text style={[type.largeTitle, { color: t.text, textAlign: 'center' }]}>Sign in to Cosmos</Text>
        <Text style={[type.body, { color: t.textSecondary, textAlign: 'center', maxWidth: 300 }]}>
          Sign in to reach your desktops and their agents from anywhere.
        </Text>
      </View>

      <Glass radius={radii.xl} style={styles.perks}>
        {PERKS.map((p, i) => <Perk key={p.icon} {...p} index={i} />)}
      </Glass>

      <View style={styles.footer}>
        {error ? <Notice kind="error" title="Sign-in didn't finish" body={error} /> : null}
        <Button title="Continue with Woozlit" icon="person.badge.key" onPress={() => void signIn()} loading={busy} />
        <Button title="Not Now" variant="ghost" onPress={leave} />
        <Text style={[type.caption, { color: t.textTertiary, textAlign: 'center', fontWeight: '400' }]}>
          Local links over Wi‑Fi work without an account — sign in to reach your desktops from anywhere.
        </Text>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, paddingHorizontal: spacing(5) },
  close: { position: 'absolute', right: spacing(4), zIndex: 2 },
  hero: { flex: 1, justifyContent: 'center', alignItems: 'center', gap: spacing(3) },
  mark: { width: 104, height: 104, borderRadius: radii.xl, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center', marginBottom: spacing(2) },
  perks: { padding: spacing(4), gap: spacing(4), marginBottom: spacing(6) },
  perk: { flexDirection: 'row', alignItems: 'center', gap: spacing(3) },
  perkIcon: { width: 40, height: 40, borderRadius: 12, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  footer: { gap: spacing(2) },
});
