import React, { useState } from 'react';
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { SymbolView } from 'expo-symbols';
import { useCosmos } from '../lib/client';
import { useTheme, spacing, radii } from '../lib/theme';

export default function SignInScreen() {
  const t = useTheme();
  const client = useCosmos();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Sign-in can be the root route when deep-linked — router.back() then
  // throws GO_BACK unhandled and swallows the completed session.
  const leave = () => {
    if (router.canGoBack()) router.back();
    else router.replace('/(tabs)');
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
    <View style={[styles.screen, { backgroundColor: t.background }]}>
      <View style={styles.body}>
        <SymbolView name="sparkles" size={44} tintColor={t.accent} />
        <Text style={[styles.title, { color: t.text }]}>Sign in to Woozlit</Text>
        <Text style={[styles.sub, { color: t.textSecondary }]}>
          Your account links this phone to your Cosmos desktops through the encrypted relay. The browser opens Woozlit — sign in there and come straight back.
        </Text>
        {error ? <Text style={[styles.err, { color: t.danger }]}>{error}</Text> : null}
        <Pressable onPress={() => void signIn()} disabled={busy} style={[styles.btn, { backgroundColor: t.accent, opacity: busy ? 0.6 : 1 }]}>
          {busy ? <ActivityIndicator color="#fff" /> : <Text style={{ color: '#fff', fontSize: 16, fontWeight: '600' }}>Continue</Text>}
        </Pressable>
        <Pressable onPress={leave} style={styles.cancel}>
          <Text style={{ color: t.textSecondary }}>Not now</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  body: { flex: 1, alignItems: 'center', justifyContent: 'center', padding: spacing(8), gap: spacing(3) },
  title: { fontSize: 24, fontWeight: '700', marginTop: spacing(2) },
  sub: { fontSize: 15, textAlign: 'center', lineHeight: 22 },
  err: { fontSize: 14, textAlign: 'center' },
  btn: { alignSelf: 'stretch', borderRadius: radii.xl, alignItems: 'center', paddingVertical: spacing(3.5), marginTop: spacing(4) },
  cancel: { padding: spacing(2) },
});
