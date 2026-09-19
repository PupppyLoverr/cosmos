import { useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View, useColorScheme } from "react-native";
import { useRouter } from "expo-router";
import { useApp } from "../../src/state/store";
import { palette } from "../../src/ui/theme";

const FALLBACK_HARNESSES = ["opencode", "claude", "codex"];
const FALLBACK_EFFORT = ["low", "medium", "high"];

function namesFrom(payload: unknown, keys: string[]): string[] {
  if (!payload) return [];
  if (Array.isArray(payload)) {
    return payload
      .map((row) => {
        if (typeof row === "string") return row;
        if (row && typeof row === "object") {
          const rec = row as Record<string, unknown>;
          for (const k of keys) {
            if (typeof rec[k] === "string") return rec[k] as string;
          }
        }
        return "";
      })
      .filter(Boolean);
  }
  if (typeof payload === "object") {
    const rec = payload as Record<string, unknown>;
    for (const k of ["harnesses", "models", "items"]) {
      if (Array.isArray(rec[k])) return namesFrom(rec[k], keys);
    }
  }
  return [];
}

export default function NewSessionScreen() {
  const app = useApp();
  const router = useRouter();
  const c = palette(useColorScheme() === "light" ? "light" : "dark");
  const [title, setTitle] = useState("");
  const [harness, setHarness] = useState("opencode");
  const [model, setModel] = useState("opencode/mimo-v2.5-free");
  const [effort, setEffort] = useState("medium");
  const [harnesses, setHarnesses] = useState<string[]>(FALLBACK_HARNESSES);
  const [models, setModels] = useState<string[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const host = app.hostSession();
    if (!host) return;
    void host
      .listHarnesses()
      .then((payload) => {
        const names = namesFrom(payload, ["id", "name", "harness"]);
        if (names.length) setHarnesses(names);
      })
      .catch(() => undefined);
  }, [app]);

  useEffect(() => {
    const host = app.hostSession();
    if (!host) return;
    void host
      .listModels(harness)
      .then((payload) => {
        const names = namesFrom(payload, ["id", "name", "model"]);
        setModels(names);
        if (names.length && !names.includes(model)) setModel(names[0]);
      })
      .catch(() => undefined);
  }, [app, harness]);

  async function create() {
    setBusy(true);
    setErr(null);
    try {
      const id = await app.createSession(
        title.trim() || "New session",
        harness.trim(),
        model.trim(),
        effort.trim(),
      );
      router.replace(`/session/${id}`);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <ScrollView style={[styles.wrap, { backgroundColor: c.bg }]} contentContainerStyle={{ paddingBottom: 40 }}>
      <Text style={[styles.title, { color: c.text }]}>New session</Text>
      <Text style={{ color: c.muted, marginBottom: 16 }}>
        Created on {app.workspace.devices.find((d) => d.id === app.selectedHostId)?.name ?? "the selected desktop"}.
        Offline desktop = offline. Harness/model lists come from that machine.
      </Text>
      <TextInput
        value={title}
        onChangeText={setTitle}
        placeholder="Title"
        placeholderTextColor={c.faint}
        style={[styles.input, { color: c.text, borderColor: c.border, backgroundColor: c.surface }]}
      />
      <Text style={[styles.label, { color: c.muted }]}>Harness</Text>
      <ChipRow c={c} values={harnesses} selected={harness} onSelect={setHarness} />
      <TextInput
        value={harness}
        onChangeText={setHarness}
        autoCapitalize="none"
        style={[styles.input, { color: c.text, borderColor: c.border, backgroundColor: c.surface }]}
      />
      <Text style={[styles.label, { color: c.muted }]}>Model</Text>
      <ChipRow c={c} values={models.slice(0, 8)} selected={model} onSelect={setModel} />
      <TextInput
        value={model}
        onChangeText={setModel}
        autoCapitalize="none"
        style={[styles.input, { color: c.text, borderColor: c.border, backgroundColor: c.surface }]}
      />
      <Text style={[styles.label, { color: c.muted }]}>Effort</Text>
      <ChipRow c={c} values={FALLBACK_EFFORT} selected={effort} onSelect={setEffort} />
      {err ? <Text style={{ color: c.danger }}>{err}</Text> : null}
      <Pressable
        onPress={create}
        disabled={busy}
        style={[styles.cta, { backgroundColor: c.accent, opacity: busy ? 0.6 : 1 }]}
      >
        <Text style={{ color: "white", fontWeight: "700" }}>{busy ? "Creating…" : "Create on desktop"}</Text>
      </Pressable>
    </ScrollView>
  );
}

function ChipRow({
  c,
  values,
  selected,
  onSelect,
}: {
  c: ReturnType<typeof palette>;
  values: string[];
  selected: string;
  onSelect: (v: string) => void;
}) {
  if (!values.length) return null;
  return (
    <View style={styles.chips}>
      {values.map((v) => (
        <Pressable
          key={v}
          onPress={() => onSelect(v)}
          style={[
            styles.chip,
            { borderColor: selected === v ? c.accent : c.border, backgroundColor: selected === v ? c.raised : "transparent" },
          ]}
        >
          <Text style={{ color: c.text, fontSize: 12 }}>{v}</Text>
        </Pressable>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: { flex: 1, padding: 20, paddingTop: 56 },
  title: { fontSize: 28, fontWeight: "700", marginBottom: 8 },
  label: { marginBottom: 6, marginTop: 8, fontSize: 13 },
  input: { borderWidth: 1, borderRadius: 14, padding: 12, marginBottom: 8 },
  cta: { marginTop: 16, borderRadius: 999, paddingVertical: 14, alignItems: "center" },
  chips: { flexDirection: "row", flexWrap: "wrap", gap: 8, marginBottom: 8 },
  chip: { borderWidth: 1, borderRadius: 999, paddingHorizontal: 10, paddingVertical: 6 },
});
