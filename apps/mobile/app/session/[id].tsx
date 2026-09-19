import { useEffect, useState } from "react";
import {
  KeyboardAvoidingView,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
  useColorScheme,
} from "react-native";
import { FlashList } from "@shopify/flash-list";
import * as ImagePicker from "expo-image-picker";
import * as DocumentPicker from "expo-document-picker";
import * as Clipboard from "expo-clipboard";
import * as FileSystem from "expo-file-system";
import { useLocalSearchParams, useRouter } from "expo-router";
import { useApp } from "../../src/state/store";
import { palette, indicatorColor } from "../../src/ui/theme";
import { bytesFromBase64 } from "../../src/protocol/upload";
import type { StagedAttachment, TranscriptItem } from "../../src/protocol/types";
import { sessionIndicator } from "../../src/protocol/types";

async function fileToBytes(uri: string): Promise<Uint8Array> {
  const b64 = await FileSystem.readAsStringAsync(uri, {
    encoding: FileSystem.EncodingType.Base64,
  });
  return bytesFromBase64(b64);
}

export default function SessionScreen() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const app = useApp();
  const router = useRouter();
  const c = palette(useColorScheme() === "light" ? "light" : "dark");
  const session = app.workspace.sessions.find((s) => s.chatId === id);
  const [items, setItems] = useState<TranscriptItem[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [pending, setPending] = useState<StagedAttachment[]>([]);

  useEffect(() => {
    const shared = app.takeShareInbox();
    if (shared.length) setPending((p) => [...p, ...shared]);
  }, [app]);

  useEffect(() => {
    if (!id) return;
    let alive = true;
    const tick = async () => {
      try {
        const rows = await app.transcript(id);
        if (alive) setItems(rows);
      } catch (e) {
        if (alive) setErr(e instanceof Error ? e.message : String(e));
      }
    };
    tick();
    const t = setInterval(tick, 1800);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [id, app]);

  async function send() {
    if (!id || (!draft.trim() && pending.length === 0)) return;
    setBusy(true);
    setErr(null);
    try {
      await app.send(id, draft.trim() || "Attached files", { attachments: pending });
      setDraft("");
      setPending([]);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    if (!id) return;
    await app.stop(id);
  }

  async function stage(uri: string, name: string, mime: string, source: StagedAttachment["source"]) {
    const bytes = await fileToBytes(uri);
    setPending((p) => [...p, { name, mime, bytes, source }]);
  }

  async function attachPhoto() {
    const res = await ImagePicker.launchImageLibraryAsync({ quality: 0.8 });
    if (res.canceled || !res.assets[0]) return;
    const a = res.assets[0];
    await stage(a.uri, a.fileName ?? "photo.jpg", a.mimeType ?? "image/jpeg", "gallery");
  }

  async function attachCamera() {
    const res = await ImagePicker.launchCameraAsync({ quality: 0.8 });
    if (res.canceled || !res.assets[0]) return;
    const a = res.assets[0];
    await stage(a.uri, a.fileName ?? "camera.jpg", a.mimeType ?? "image/jpeg", "camera");
  }

  async function attachFile() {
    const res = await DocumentPicker.getDocumentAsync({});
    if (res.canceled || !res.assets[0]) return;
    const a = res.assets[0];
    await stage(a.uri, a.name ?? "file.bin", a.mimeType ?? "application/octet-stream", "file");
  }

  async function attachClipboard() {
    const text = await Clipboard.getStringAsync();
    if (text) setDraft((d) => (d ? `${d}\n${text}` : text));
  }

  const ind = session ? sessionIndicator(session, Date.now()) : "idle";

  return (
    <KeyboardAvoidingView
      style={{ flex: 1, backgroundColor: c.bg }}
      behavior={Platform.OS === "android" ? "height" : "padding"}
    >
      <View style={[styles.top, { borderBottomColor: c.border }]}>
        <Pressable onPress={() => router.back()}>
          <Text style={{ color: c.accent, fontWeight: "600" }}>Back</Text>
        </Pressable>
        <View style={{ flex: 1, marginHorizontal: 12 }}>
          <Text numberOfLines={1} style={{ color: c.text, fontWeight: "700" }}>
            {session?.title ?? "Session"}
          </Text>
          <Text numberOfLines={1} style={{ color: c.muted, fontSize: 12 }}>
            {session?.harness} · {session?.model}
          </Text>
        </View>
        <View style={[styles.dot, { backgroundColor: indicatorColor[ind] }]} />
      </View>
      <FlashList
        data={items}
        drawDistance={600}
        keyExtractor={(it, i) => it.id || String(i)}
        contentContainerStyle={{ padding: 16, paddingBottom: 24 }}
        ListEmptyComponent={
          <Text style={{ color: c.muted, padding: 16 }}>
            Transcript loads from the host chat2 tail. Empty means the desktop has not published one yet — not a mock.
          </Text>
        }
        renderItem={({ item }) => (
          <View
            style={[
              styles.bubble,
              item.role === "user"
                ? { alignSelf: "flex-end", backgroundColor: c.accent }
                : { alignSelf: "flex-start", backgroundColor: c.raised },
            ]}
          >
            <Text style={{ color: item.role === "user" ? "white" : c.text }}>{item.text}</Text>
          </View>
        )}
      />
      {err ? <Text style={{ color: c.danger, paddingHorizontal: 16 }}>{err}</Text> : null}
      {pending.length ? (
        <Text style={{ color: c.muted, paddingHorizontal: 16 }}>
          {pending.map((p) => p.name).join(", ")} will upload to this desktop on Send.
        </Text>
      ) : null}
      <View style={[styles.composer, { borderTopColor: c.border, backgroundColor: c.surface }]}>
        <Pressable onPress={attachPhoto} style={styles.iconBtn}>
          <Text style={{ color: c.muted }}>+</Text>
        </Pressable>
        <TextInput
          value={draft}
          onChangeText={setDraft}
          placeholder="Message"
          placeholderTextColor={c.faint}
          multiline
          style={[styles.field, { color: c.text }]}
        />
        {session?.status === "working" ? (
          <Pressable onPress={stop} style={[styles.send, { backgroundColor: c.danger }]}>
            <Text style={{ color: "white", fontWeight: "700" }}>Stop</Text>
          </Pressable>
        ) : (
          <Pressable
            onPress={send}
            disabled={busy}
            style={[styles.send, { backgroundColor: c.accent, opacity: busy ? 0.6 : 1 }]}
          >
            <Text style={{ color: "white", fontWeight: "700" }}>{busy ? "…" : "Send"}</Text>
          </Pressable>
        )}
      </View>
      <View style={styles.attachRow}>
        <Pressable onPress={attachFile}>
          <Text style={{ color: c.accent }}>File</Text>
        </Pressable>
        <Pressable onPress={attachClipboard}>
          <Text style={{ color: c.accent }}>Clipboard</Text>
        </Pressable>
        <Pressable onPress={attachCamera}>
          <Text style={{ color: c.accent }}>Camera</Text>
        </Pressable>
      </View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  top: {
    paddingTop: 52,
    paddingHorizontal: 16,
    paddingBottom: 12,
    flexDirection: "row",
    alignItems: "center",
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  dot: { width: 8, height: 8, borderRadius: 4 },
  bubble: { maxWidth: "82%", borderRadius: 16, padding: 12, marginBottom: 8 },
  composer: {
    flexDirection: "row",
    alignItems: "flex-end",
    padding: 10,
    gap: 8,
    borderTopWidth: StyleSheet.hairlineWidth,
  },
  iconBtn: { width: 36, height: 36, alignItems: "center", justifyContent: "center" },
  field: { flex: 1, maxHeight: 120, fontSize: 16, paddingVertical: 8 },
  send: { borderRadius: 999, paddingHorizontal: 16, paddingVertical: 10 },
  attachRow: { flexDirection: "row", gap: 16, paddingHorizontal: 16, paddingBottom: 12 },
});
