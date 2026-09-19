import { useState } from "react";
import {
  KeyboardAvoidingView,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from "react-native";
import { useRouter } from "expo-router";
import { useColorScheme } from "react-native";
import { useApp } from "../src/state/store";
import { palette } from "../src/ui/theme";
import {
  DEFAULT_EDGE,
  authModeFromInput,
  devBearer,
  exchangeWoozlitCustomToken,
  isFirebaseIdToken,
  jwtSub,
  newDeviceId,
} from "../src/protocol/auth";

export default function SignInScreen() {
  const scheme = useColorScheme() === "light" ? "light" : "dark";
  const c = palette(scheme);
  const app = useApp();
  const router = useRouter();
  const [mode, setMode] = useState<"woozlit" | "dev">("woozlit");
  const [edge, setEdge] = useState(DEFAULT_EDGE);
  const [token, setToken] = useState("");
  const [userId, setUserId] = useState("");
  const [orgId, setOrgId] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  async function submit() {
    setBusy(true);
    setErr(null);
    try {
      const deviceId = newDeviceId();
      if (mode === "dev") {
        const user = userId.trim();
        const org = orgId.trim() || user;
        await app.signIn({
          edgeUrl: edge.trim(),
          userId: user,
          orgId: org,
          token: devBearer(user, org),
          deviceId,
          deviceName: "Android",
          mode: "dev",
        });
      } else {
        const raw = token.trim();
        if (!raw) throw new Error("Paste the Woozlit token from the login bounce");
        let access = raw;
        let refresh: string | undefined;
        if (!isFirebaseIdToken(raw)) {
          const exchanged = await exchangeWoozlitCustomToken(raw);
          access = exchanged.idToken;
          refresh = exchanged.refreshToken;
        }
        const sub = jwtSub(access) ?? "woozlit-user";
        await app.signIn({
          edgeUrl: edge.trim(),
          userId: sub,
          orgId: sub,
          token: access,
          refreshToken: refresh,
          deviceId,
          deviceName: "Android",
          mode: "woozlit",
        });
      }
      router.replace("/(app)");
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <KeyboardAvoidingView
      style={[styles.wrap, { backgroundColor: c.bg }]}
      behavior={Platform.OS === "ios" ? "padding" : undefined}
    >
      <View style={styles.hero}>
        <View style={[styles.mark, { backgroundColor: c.accent }]} />
        <Text style={[styles.title, { color: c.text }]}>Cosmos</Text>
        <Text style={[styles.sub, { color: c.muted }]}>
          Your desktops. Real sessions. No cloud execution.
        </Text>
      </View>
      <View style={styles.tabs}>
        {(["woozlit", "dev"] as const).map((m) => (
          <Pressable
            key={m}
            onPress={() => setMode(m)}
            style={[
              styles.tab,
              { borderColor: c.border, backgroundColor: mode === m ? c.raised : "transparent" },
            ]}
          >
            <Text style={{ color: mode === m ? c.text : c.muted, fontWeight: "600" }}>
              {m === "woozlit" ? "Woozlit" : "Dev edge"}
            </Text>
          </Pressable>
        ))}
      </View>
      <TextInput
        value={edge}
        onChangeText={setEdge}
        autoCapitalize="none"
        autoCorrect={false}
        placeholder="Edge URL"
        placeholderTextColor={c.faint}
        style={[styles.input, { color: c.text, borderColor: c.border, backgroundColor: c.surface }]}
      />
      {mode === "woozlit" ? (
        <TextInput
          value={token}
          onChangeText={setToken}
          autoCapitalize="none"
          autoCorrect={false}
          placeholder="Woozlit token (never logged)"
          placeholderTextColor={c.faint}
          secureTextEntry
          style={[styles.input, { color: c.text, borderColor: c.border, backgroundColor: c.surface }]}
        />
      ) : (
        <>
          <TextInput
            value={userId}
            onChangeText={setUserId}
            autoCapitalize="none"
            placeholder="User ID"
            placeholderTextColor={c.faint}
            style={[styles.input, { color: c.text, borderColor: c.border, backgroundColor: c.surface }]}
          />
          <TextInput
            value={orgId}
            onChangeText={setOrgId}
            autoCapitalize="none"
            placeholder="Org ID"
            placeholderTextColor={c.faint}
            style={[styles.input, { color: c.text, borderColor: c.border, backgroundColor: c.surface }]}
          />
        </>
      )}
      {err ? <Text style={{ color: c.danger, marginBottom: 12 }}>{err}</Text> : null}
      <Pressable
        onPress={submit}
        disabled={busy}
        style={[styles.cta, { backgroundColor: c.accent, opacity: busy ? 0.6 : 1 }]}
      >
        <Text style={styles.ctaText}>{busy ? "Connecting…" : "Sign in"}</Text>
      </Pressable>
      <Text style={[styles.foot, { color: c.faint }]}>
        Woozlit credentials are stored on-device. They are never committed or screenshot.
      </Text>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  wrap: { flex: 1, padding: 24, justifyContent: "center" },
  hero: { marginBottom: 28 },
  mark: { width: 36, height: 36, borderRadius: 12, marginBottom: 16 },
  title: { fontSize: 32, fontWeight: "700", letterSpacing: -0.6 },
  sub: { marginTop: 8, fontSize: 16, lineHeight: 22 },
  tabs: { flexDirection: "row", gap: 8, marginBottom: 16 },
  tab: { flex: 1, paddingVertical: 10, borderRadius: 999, borderWidth: 1, alignItems: "center" },
  input: {
    borderWidth: 1,
    borderRadius: 14,
    paddingHorizontal: 14,
    paddingVertical: 12,
    marginBottom: 10,
    fontSize: 16,
  },
  cta: { marginTop: 8, borderRadius: 999, paddingVertical: 14, alignItems: "center" },
  ctaText: { color: "white", fontWeight: "700", fontSize: 16 },
  foot: { marginTop: 16, fontSize: 12, lineHeight: 18 },
});
