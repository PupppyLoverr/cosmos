import { useMemo, useState } from "react";
import { Pressable, StyleSheet, Text, TextInput, View, useColorScheme } from "react-native";
import { FlashList } from "@shopify/flash-list";
import { useRouter } from "expo-router";
import { useApp } from "../../src/state/store";
import { palette, indicatorColor } from "../../src/ui/theme";
import { sessionIndicator } from "../../src/protocol/types";
import { androidFabElevation, androidRipple } from "../../src/ui/press";

export default function SessionsScreen() {
  const app = useApp();
  const router = useRouter();
  const c = palette(useColorScheme() === "light" ? "light" : "dark");
  const [q, setQ] = useState("");
  const now = Date.now();
  const rows = useMemo(() => {
    const host = app.selectedHostId;
    return app.workspace.sessions
      .filter((s) => !host || s.deviceId === host)
      .filter((s) => {
        if (!q.trim()) return true;
        const hay = `${s.title} ${s.harness ?? ""} ${s.model ?? ""}`.toLowerCase();
        return hay.includes(q.trim().toLowerCase());
      })
      .sort((a, b) => (b.lastMessageAt ?? b.createdAt) - (a.lastMessageAt ?? a.createdAt));
  }, [app.workspace.sessions, app.selectedHostId, q]);

  const host = app.workspace.devices.find((d) => d.id === app.selectedHostId);

  return (
    <View style={[styles.wrap, { backgroundColor: c.bg }]}>
      <View style={styles.head}>
        <Text style={[styles.title, { color: c.text }]}>Sessions</Text>
        <Text style={{ color: c.muted }}>{host?.name ?? "No desktop"}</Text>
      </View>
      <TextInput
        value={q}
        onChangeText={setQ}
        placeholder="Search"
        placeholderTextColor={c.faint}
        style={[styles.search, { color: c.text, backgroundColor: c.surface, borderColor: c.border }]}
      />
      <FlashList
        data={rows}
        drawDistance={400}
        keyExtractor={(s) => s.chatId}
        contentContainerStyle={{ paddingBottom: 100 }}
        ListEmptyComponent={
          <Text style={{ color: c.muted, padding: 24 }}>
            {app.connection
              ? "No sessions on this desktop yet. Open New to create one — the list is live from the registry, not a demo."
              : "Sign in to load real sessions."}
          </Text>
        }
        renderItem={({ item }) => {
          const ind = sessionIndicator(item, now);
          return (
            <Pressable
              onPress={() => router.push(`/session/${item.chatId}`)}
              android_ripple={androidRipple(c.raised)}
              style={[styles.row, { borderBottomColor: c.border }]}
            >
              <View style={[styles.dot, { backgroundColor: indicatorColor[ind] ?? c.idle }]} />
              <View style={{ flex: 1 }}>
                <Text numberOfLines={1} style={[styles.rowTitle, { color: c.text }]}>
                  {item.title}
                </Text>
                <Text numberOfLines={1} style={{ color: c.muted, fontSize: 13 }}>
                  {item.lastMessagePreview || `${item.harness ?? "agent"} · ${item.model ?? ""}`}
                </Text>
              </View>
            </Pressable>
          );
        }}
      />
      <Pressable
        onPress={() => router.push("/(app)/new")}
        android_ripple={androidRipple("#ffffff40", true)}
        style={[styles.fab, { backgroundColor: c.accent, elevation: androidFabElevation }]}
      >
        <Text style={styles.fabText}>New</Text>
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: { flex: 1, paddingTop: 56 },
  head: { paddingHorizontal: 20, marginBottom: 12 },
  title: { fontSize: 28, fontWeight: "700", letterSpacing: -0.4 },
  search: {
    marginHorizontal: 20,
    marginBottom: 8,
    borderRadius: 12,
    borderWidth: 1,
    paddingHorizontal: 14,
    paddingVertical: 10,
  },
  row: {
    flexDirection: "row",
    alignItems: "center",
    gap: 12,
    paddingHorizontal: 20,
    paddingVertical: 14,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  rowTitle: { fontSize: 16, fontWeight: "600", marginBottom: 2 },
  dot: { width: 10, height: 10, borderRadius: 5 },
  fab: {
    position: "absolute",
    right: 20,
    bottom: 24,
    borderRadius: 999,
    paddingHorizontal: 22,
    paddingVertical: 14,
  },
  fabText: { color: "white", fontWeight: "700" },
});
