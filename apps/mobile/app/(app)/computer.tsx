import { useEffect, useRef, useState } from "react";
import {
  Image,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
  useColorScheme,
} from "react-native";
import { useApp } from "../../src/state/store";
import { palette } from "../../src/ui/theme";
import { androidRipple } from "../../src/ui/press";

export default function ComputerScreen() {
  const app = useApp();
  const c = palette(useColorScheme() === "light" ? "light" : "dark");
  const [status, setStatus] = useState<string>("");
  const [activity, setActivity] = useState<string>("");
  const [live, setLive] = useState(false);
  const [shot, setShot] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const stopLive = useRef<(() => void) | null>(null);
  const host = app.hostSession();
  const paired = Boolean(app.selectedHostId && app.pairing[app.selectedHostId]);
  const desktop = app.workspace.devices.find((d) => d.id === app.selectedHostId);

  useEffect(() => {
    return () => {
      stopLive.current?.();
    };
  }, []);

  async function refreshStatus() {
    setErr(null);
    try {
      const s = await host?.computerStatus();
      setStatus(JSON.stringify(s, null, 2));
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }

  async function startTask() {
    setErr(null);
    try {
      const result = await host?.computerObserve({
        scope: "frontmost",
        includeScreenshot: false,
        maxElements: 200,
      });
      setActivity(JSON.stringify(result, null, 2));
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }

  async function watchLive() {
    if (!host) return;
    stopLive.current?.();
    setLive(true);
    setErr(null);
    try {
      stopLive.current = await host.watchLive(async (item) => {
        setActivity(JSON.stringify(item, null, 2).slice(0, 4000));
        if (item && typeof item === "object") {
          const rec = item as {
            screenshot?: { path?: string };
            text?: string;
          };
          if (rec.screenshot?.path) {
            try {
              const uri = await host.readAttachmentDataUrl(rec.screenshot.path);
              setShot(uri);
            } catch {
              /* keep last frame */
            }
          }
        }
      });
    } catch (e) {
      setLive(false);
      setErr(e instanceof Error ? e.message : String(e));
    }
  }

  async function decide(decision: "approve" | "deny" | "pause" | "stop") {
    setErr(null);
    try {
      if (decision === "stop") {
        stopLive.current?.();
        stopLive.current = null;
        setLive(false);
        await host?.computerStop("stopped from Android");
      }
      const result = await host?.computerDecide(decision, `${decision} from Android`);
      setActivity(JSON.stringify(result, null, 2));
      if (decision === "stop") setActivity("stopped — desktop Computer control halted");
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <ScrollView style={{ flex: 1, backgroundColor: c.bg }} contentContainerStyle={{ padding: 20, paddingTop: 56 }}>
      <Text style={{ color: c.text, fontSize: 28, fontWeight: "700" }}>Computer Use</Text>
      <Text style={{ color: c.muted, marginVertical: 8 }}>
        {desktop ? `${desktop.name} · ${desktop.platform}` : "Select a desktop"} — STOP actually stops desktop control. Watch Live streams only while this screen is open.
      </Text>
      {!paired ? (
        <Text style={{ color: c.warning, marginBottom: 12 }}>
          Pair this phone on the Desktops tab first. The desktop must Approve.
        </Text>
      ) : null}
      <View style={styles.row}>
        <Btn c={c} label="Status" onPress={refreshStatus} />
        <Btn c={c} label="Start" onPress={startTask} />
        <Btn c={c} label={live ? "Watching" : "Watch Live"} onPress={watchLive} />
        <Btn c={c} label="STOP" danger onPress={() => decide("stop")} />
      </View>
      <View style={styles.row}>
        <Btn c={c} label="Approve" onPress={() => decide("approve")} />
        <Btn c={c} label="Deny" onPress={() => decide("deny")} />
        <Btn c={c} label="Pause" onPress={() => decide("pause")} />
      </View>
      {err ? <Text style={{ color: c.danger, marginTop: 8 }}>{err}</Text> : null}
      {shot ? (
        <Image source={{ uri: shot }} style={{ height: 180, borderRadius: 12, marginTop: 12 }} />
      ) : (
        <View style={[styles.preview, { borderColor: c.border, backgroundColor: c.surface }]}>
          <Text style={{ color: c.faint }}>
            {live
              ? "Watch Live subscribed — frames only while this screen is open."
              : "Low-bandwidth: events + on-demand screenshots until you tap Watch Live."}
          </Text>
        </View>
      )}
      {activity ? (
        <Text selectable style={{ color: c.muted, marginTop: 12, fontFamily: "monospace", fontSize: 12 }}>
          {activity.slice(0, 4000)}
        </Text>
      ) : null}
      {status ? (
        <Text selectable style={{ color: c.faint, marginTop: 12, fontFamily: "monospace", fontSize: 11 }}>
          {status.slice(0, 2500)}
        </Text>
      ) : null}
    </ScrollView>
  );
}

function Btn({
  c,
  label,
  onPress,
  danger,
}: {
  c: ReturnType<typeof palette>;
  label: string;
  onPress: () => void;
  danger?: boolean;
}) {
  return (
    <Pressable
      onPress={onPress}
      android_ripple={androidRipple(danger ? "#ffffff30" : c.raised)}
      style={{
        borderWidth: 1,
        borderColor: danger ? c.danger : c.border,
        borderRadius: 999,
        paddingHorizontal: 12,
        paddingVertical: 8,
      }}
    >
      <Text style={{ color: danger ? c.danger : c.text, fontWeight: "600" }}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  row: { flexDirection: "row", flexWrap: "wrap", gap: 8, marginTop: 8 },
  preview: {
    marginTop: 16,
    borderWidth: 1,
    borderRadius: 16,
    height: 160,
    alignItems: "center",
    justifyContent: "center",
    padding: 16,
  },
});
