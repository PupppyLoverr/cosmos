import React, { useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useCosmos } from '../lib/client';
import { useTheme, spacing, type, tintFor } from '../lib/theme';
import { Button } from '../components/bits';
import { Screen } from '../components/surface';

// Scattered tinted marks, echoing the desktop's project badges.
const MARKS: { x: number; y: number; s: number; r: number; icon: string }[] = [
  { x: 0.12, y: 0.10, s: 56, r: 22, icon: 'sparkles' },
  { x: 0.72, y: 0.06, s: 44, r: 22, icon: 'display' },
  { x: 0.84, y: 0.30, s: 50, r: 16, icon: 'folder' },
  { x: 0.05, y: 0.42, s: 42, r: 21, icon: 'terminal' },
  { x: 0.70, y: 0.58, s: 54, r: 18, icon: 'cursorarrow.click.2' },
  { x: 0.18, y: 0.66, s: 46, r: 23, icon: 'bolt' },
];

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
    <Screen style={[styles.screen, { paddingTop: insets.top + spacing(2), paddingBottom: insets.bottom + spacing(4) }]}>
      <View style={styles.hero}>
        {MARKS.map((m) => (
          <View
            key={m.icon}
            style={{
              position: 'absolute', left: `${m.x * 100}%`, top: `${m.y * 100}%`,
              width: m.s, height: m.s, borderRadius: m.r, borderCurve: 'continuous',
              backgroundColor: tintFor(m.icon), alignItems: 'center', justifyContent: 'center',
            }}
          >
            <SymbolView name={m.icon as never} size={m.s * 0.42} tintColor="#fff" weight="semibold" />
          </View>
        ))}
        <View style={styles.heroText}>
          <Text style={[type.largeTitle, { color: t.text, textAlign: 'center' }]}>Cosmos</Text>
          <Text style={[type.subhead, { color: t.textSecondary, textAlign: 'center', maxWidth: 260 }]}>
            Your Mac, your agents — driven from your pocket.
          </Text>
        </View>
      </View>

      <View style={styles.footer}>
        {error ? <Text style={[type.footnote, { color: t.danger, textAlign: 'center' }]}>{error}</Text> : null}
        <Button title="Sign in with Woozlit" onPress={() => void signIn()} loading={busy} />
        <Button title="Not now" variant="ghost" onPress={leave} />
        <Text style={[type.caption, { color: t.textTertiary, textAlign: 'center', fontWeight: '400' }]}>
          Signing in links this phone to your desktops over the encrypted relay. Local links work without an account.
        </Text>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, paddingHorizontal: spacing(6) },
  hero: { flex: 1, justifyContent: 'center' },
  heroText: { alignItems: 'center', gap: spacing(2) },
  footer: { gap: spacing(2) },
});
