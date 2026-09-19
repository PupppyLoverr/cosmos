import { useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View, useColorScheme } from "react-native";
import { useApp } from "../../src/state/store";
import { palette } from "../../src/ui/theme";
import { androidRipple } from "../../src/ui/press";
import {
  computerCapable,
  deviceOnline,
  formatLastSeen,
  trustLabel,
  type TrustPublic,
} from "../../src/protocol/types";

export default function DevicesScreen() {
  const app = useApp();
  const c = palette(useColorScheme() === "light" ? "light" : "dark");
  const now = Date.now();
  const [status, setStatus] = useState<{
    pendingTrust?: TrustPublic[];
    trustedDevices?: TrustPublic[];
  } | null>(null);

  useEffect(() => {
    let alive = true;
    const tick = async () => {
      try {
        const s = (await app.hostSession()?.computerStatus()) as typeof status;
        if (alive) setStatus(s ?? null);
      } catch {
        if (alive) setStatus(null);
      }
    };
    tick();
    const t = setInterval(tick, 4000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [app, app.selectedHostId, app.pairing]);

  return (
    <ScrollView style={[styles.wrap, { backgroundColor: c.bg }]} contentContainerStyle={{ padding: 20, paddingTop: 56 }}>
      <Text style={[styles.title, { color: c.text }]}>Desktops</Text>
      <Text style={{ color: c.muted, marginBottom: 16 }}>
        Same-account discovery. Computer Use requires an explicit pairing on the desktop.
      </Text>
      {app.workspace.devices.length === 0 ? (
        <Text style={{ color: c.muted }}>No devices on this account. The list is empty until the registry says otherwise.</Text>
      ) : null}
      {app.workspace.devices.map((d) => {
        const online = deviceOnline(d, now, app.workspace.presence);
        const selected = app.selectedHostId === d.id;
        const trustedRemote = Boolean(
          app.connection &&
            status?.trustedDevices?.some((row) => row.deviceId === app.connection?.deviceId),
        );
        const pendingRemote = Boolean(
          app.connection &&
            status?.pendingTrust?.some((row) => row.deviceId === app.connection?.deviceId),
        );
        const trust = trustLabel({
          hasToken: Boolean(app.pairing[d.id]),
          pending: pendingRemote && selected,
          trusted: trustedRemote && selected,
        });
        const sessions = app.workspace.sessions.filter((s) => s.deviceId === d.id).length;
        return (
          <Pressable
            key={d.id}
            onPress={() => app.selectHost(d.id)}
            android_ripple={androidRipple(c.raised)}
            style={[
              styles.card,
              {
                backgroundColor: c.surface,
                borderColor: selected ? c.accent : c.border,
              },
            ]}
          >
            <View style={styles.row}>
              <View style={[styles.dot, { backgroundColor: online ? c.done : c.idle }]} />
              <View style={{ flex: 1 }}>
                <Text style={{ color: c.text, fontWeight: "700", fontSize: 16 }}>{d.name}</Text>
                <Text style={{ color: c.muted, marginTop: 2 }}>
                  {d.platform || "unknown"} · {online ? "online" : "offline"} · last seen{" "}
                  {formatLastSeen(app.workspace.presence[d.id] ?? d.lastSeenAt, now)} · {sessions}{" "}
                  sessions
                </Text>
                <Text style={{ color: c.faint, marginTop: 2 }}>
                  Computer Use: {computerCapable(d) ? "capable" : "unknown"} · trust: {trust}
                </Text>
              </View>
            </View>
            <View style={styles.actions}>
              <Pressable
                onPress={() => app.requestPairing()}
                android_ripple={androidRipple(c.raised)}
                style={[styles.btn, { borderColor: c.border }]}
              >
                <Text style={{ color: c.text }}>Request trust</Text>
              </Pressable>
              {app.pairing[d.id] ? (
                <Pressable
                  onPress={() => {
                    void app.revokePairing(d.id);
                  }}
                  style={[styles.btn, { borderColor: c.danger }]}
                >
                  <Text style={{ color: c.danger }}>Revoke</Text>
                </Pressable>
              ) : null}
            </View>
          </Pressable>
        );
      })}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  wrap: { flex: 1 },
  title: { fontSize: 28, fontWeight: "700", marginBottom: 8 },
  card: { borderWidth: 1, borderRadius: 16, padding: 16, marginBottom: 12 },
  row: { flexDirection: "row", gap: 12, alignItems: "flex-start" },
  dot: { width: 10, height: 10, borderRadius: 5, marginTop: 6 },
  actions: { flexDirection: "row", gap: 8, marginTop: 12 },
  btn: { borderWidth: 1, borderRadius: 999, paddingHorizontal: 12, paddingVertical: 8 },
});
