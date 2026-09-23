import React, { useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useCosmos } from '../lib/client';
import { useTheme, spacing, type } from '../lib/theme';
import { Button, IconButton, Notice } from '../components/bits';
import { Screen } from '../components/surface';
import { HarnessMark } from '../components/harness-mark';

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
        <IconButton icon="xmark" label="Close" tint={t.textSecondary} onPress={leave} />
      </View>
      <View style={styles.hero}>
        <HarnessMark size={44} plain />
        <Text accessibilityRole="header" style={[type.title, { color: t.text, textAlign: 'center' }]}>Sign in to Cosmos</Text>
      </View>

      <View style={styles.footer}>
        {error ? <Notice kind="error" title="Sign-in didn't finish" body={error} /> : null}
        <Button title="Continue with Woozlit" onPress={() => void signIn()} loading={busy} />
        <Text style={[type.footnote, { color: t.textSecondary, textAlign: 'center' }]}>
          Your desktops only accept phones they have paired with.
        </Text>
      </View>
    </Screen>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, paddingHorizontal: spacing(5) },
  close: { position: 'absolute', right: spacing(4), zIndex: 2 },
  hero: { flex: 1, justifyContent: 'center', alignItems: 'center', gap: spacing(5) },
  footer: { gap: spacing(3) },
});
