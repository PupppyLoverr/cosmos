import { useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View, useColorScheme } from "react-native";
import { useRouter } from "expo-router";
import { useApp } from "../../src/state/store";
import { palette } from "../../src/ui/theme";
import { hasRemotePushCredential } from "../../src/notify";
import { deliveryForPush } from "../../src/protocol/notices";

export default function SettingsScreen() {
  const app = useApp();
  const router = useRouter();
  const scheme = useColorScheme() === "light" ? "light" : "dark";
  const c = palette(scheme);
  const push = deliveryForPush(hasRemotePushCredential());
  const [composio, setComposio] = useState("");
  const [composioState, setComposioState] = useState<string>("not connected from this phone");
  const [err, setErr] = useState<string | null>(null);

  async function connectComposio() {
    setErr(null);
    const key = composio.trim();
    if (!key) {
      setErr("Paste a Composio API key supplied at runtime. It is not stored in git.");
      return;
    }
    try {
      const host = app.hostSession();
      if (!host) throw new Error("Pick a desktop first");
      await host.connectComposio(key);
      setComposio("");
      setComposioState("connected on the selected desktop (key not stored on phone)");
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <ScrollView style={{ flex: 1, backgroundColor: c.bg }} contentContainerStyle={{ padding: 20, paddingTop: 56 }}>
      <Text style={{ color: c.text, fontSize: 28, fontWeight: "700" }}>Settings</Text>
      <Row c={c} k="Signed in" v={app.connection ? `${app.connection.mode} · ${app.connection.userId}` : "no"} />
      <Row c={c} k="This device" v={app.connection?.deviceId ?? "—"} />
      <Row c={c} k="Edge" v={app.connection?.edgeUrl ?? "—"} />
      <Row c={c} k="Appearance" v={scheme} />
      <Row c={c} k="Push delivery" v={push} />
      <Text style={{ color: c.faint, marginTop: 8, fontSize: 12 }}>
        Remote push reports credential-missing until EXPO_PUBLIC_FCM_TOKEN is supplied at runtime.
        Local notifications still fire for task complete, approval, Computer Use stop, and offline.
      </Text>
      <Text style={{ color: c.text, fontWeight: "700", marginTop: 24 }}>Composio</Text>
      <Text style={{ color: c.muted, marginVertical: 6 }}>{composioState}</Text>
      <TextInput
        value={composio}
        onChangeText={setComposio}
        autoCapitalize="none"
        autoCorrect={false}
        secureTextEntry
        placeholder="Composio API key (runtime only)"
        placeholderTextColor={c.faint}
        style={{
          borderWidth: 1,
          borderColor: c.border,
          backgroundColor: c.surface,
          color: c.text,
          borderRadius: 14,
          padding: 12,
        }}
      />
      <Pressable onPress={connectComposio} style={[styles.btn, { borderColor: c.border, marginTop: 12 }]}>
        <Text style={{ color: c.text, fontWeight: "700" }}>Connect on desktop</Text>
      </Pressable>
      {err ? <Text style={{ color: c.danger, marginTop: 8 }}>{err}</Text> : null}
      <Pressable
        onPress={() => {
          app.signOut();
          router.replace("/");
        }}
        style={[styles.btn, { borderColor: c.danger, marginTop: 24 }]}
      >
        <Text style={{ color: c.danger, fontWeight: "700" }}>Sign out</Text>
      </Pressable>
    </ScrollView>
  );
}

function Row({ c, k, v }: { c: ReturnType<typeof palette>; k: string; v: string }) {
  return (
    <View style={{ paddingVertical: 12, borderBottomWidth: StyleSheet.hairlineWidth, borderBottomColor: c.border }}>
      <Text style={{ color: c.faint, fontSize: 12 }}>{k}</Text>
      <Text style={{ color: c.text, marginTop: 4 }}>{v}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  btn: { borderWidth: 1, borderRadius: 999, paddingVertical: 12, alignItems: "center" },
});
