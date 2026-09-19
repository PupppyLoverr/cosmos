import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import * as SecureStore from "expo-secure-store";
import type {
  Connection,
  DeviceInfo,
  SessionSummary,
  SpaceInfo,
  StagedAttachment,
  TranscriptItem,
} from "../protocol/types";
import { EdgeClient } from "../protocol/edge";
import { HostSession } from "../protocol/host";
import { randomPairingToken } from "../protocol/computer";
import { forgetPairing, rememberPairing, type PairingMap } from "../protocol/notices";
import { presentComputerNotice } from "../notify";
import type { NoticeKind } from "../protocol/types";

const CONN_KEY = "cosmos.connection";
const PAIR_KEY = "cosmos.pairing";

interface Workspace {
  sessions: SessionSummary[];
  spaces: SpaceInfo[];
  devices: DeviceInfo[];
  presence: Record<string, number>;
}

interface AppState {
  connection: Connection | null;
  workspace: Workspace;
  selectedHostId: string | null;
  pairing: PairingMap;
  error: string | null;
  linking: boolean;
  restoring: boolean;
  signIn: (conn: Connection) => Promise<void>;
  signOut: () => void;
  refresh: () => Promise<void>;
  selectHost: (id: string) => void;
  hostSession: () => HostSession | null;
  requestPairing: () => Promise<void>;
  revokePairing: (hostId: string) => void;
  send: (
    chatId: string,
    prompt: string,
    extras?: {
      harness?: string | null;
      model?: string | null;
      attachments?: StagedAttachment[];
    },
  ) => Promise<void>;
  stop: (chatId: string) => Promise<void>;
  transcript: (chatId: string) => Promise<TranscriptItem[]>;
  createSession: (title: string, harness: string, model: string, reasoning?: string) => Promise<string>;
}

const Ctx = createContext<AppState | null>(null);

async function readJson<T>(key: string): Promise<T | null> {
  try {
    const raw = await SecureStore.getItemAsync(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

async function writeJson(key: string, value: unknown): Promise<void> {
  await SecureStore.setItemAsync(key, JSON.stringify(value));
}

export function AppProvider({ children }: { children: React.ReactNode }) {
  const [connection, setConnection] = useState<Connection | null>(null);
  const [workspace, setWorkspace] = useState<Workspace>({
    sessions: [],
    spaces: [],
    devices: [],
    presence: {},
  });
  const [selectedHostId, setSelectedHostId] = useState<string | null>(null);
  const [pairing, setPairing] = useState<PairingMap>({});
  const [error, setError] = useState<string | null>(null);
  const [linking, setLinking] = useState(false);
  const [restoring, setRestoring] = useState(true);
  const edge = useRef<EdgeClient | null>(null);
  const hosts = useRef<Map<string, HostSession>>(new Map());
  const eventUnsub = useRef<(() => void) | null>(null);

  const hydrate = useCallback(async (client: EdgeClient) => {
    await client.pull();
    const devices = client.devices();
    setWorkspace({
      sessions: client.sessions(),
      spaces: client.spaces(),
      devices,
      presence: client.presence,
    });
    setSelectedHostId((prev) => prev ?? devices[0]?.id ?? null);
  }, []);

  const listenHostEvents = useCallback(async (host: HostSession) => {
    eventUnsub.current?.();
    try {
      eventUnsub.current = await host.watchEvents(async (item) => {
        if (!item || typeof item !== "object") return;
        const n = item as { kind?: string; title?: string; body?: string };
        if (!n.kind || n.kind === "idle") return;
        const kind = (
          n.kind === "computer-foreground" ? "foreground-escalation" : n.kind
        ) as NoticeKind;
        await presentComputerNotice({
          kind,
          title: n.title ?? n.kind,
          body: n.body ?? "",
        });
      });
    } catch {
      eventUnsub.current = null;
    }
  }, []);

  const signIn = useCallback(
    async (conn: Connection) => {
      setLinking(true);
      setError(null);
      try {
        const client = new EdgeClient(conn);
        await client.health();
        await hydrate(client);
        edge.current = client;
        setConnection(conn);
        await writeJson(CONN_KEY, conn);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        throw e;
      } finally {
        setLinking(false);
      }
    },
    [hydrate],
  );

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const savedPair = await readJson<PairingMap>(PAIR_KEY);
        if (alive && savedPair) setPairing(savedPair);
        const saved = await readJson<Connection>(CONN_KEY);
        if (alive && saved?.token && saved.edgeUrl) {
          try {
            await signIn(saved);
          } catch {
            /* stay on sign-in; token may be stale */
          }
        }
      } finally {
        if (alive) setRestoring(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [signIn]);

  useEffect(() => {
    void writeJson(PAIR_KEY, pairing);
  }, [pairing]);

  const signOut = useCallback(() => {
    eventUnsub.current?.();
    eventUnsub.current = null;
    for (const h of hosts.current.values()) h.close();
    hosts.current.clear();
    edge.current = null;
    setConnection(null);
    setWorkspace({ sessions: [], spaces: [], devices: [], presence: {} });
    setSelectedHostId(null);
    void SecureStore.deleteItemAsync(CONN_KEY);
  }, []);

  const refresh = useCallback(async () => {
    if (!edge.current) return;
    await hydrate(edge.current);
  }, [hydrate]);

  const hostSession = useCallback((): HostSession | null => {
    if (!connection || !selectedHostId) return null;
    const token = pairing[selectedHostId] ?? null;
    let h = hosts.current.get(selectedHostId);
    if (!h || h.pairingToken !== token) {
      h?.close();
      h = new HostSession(connection, selectedHostId, token);
      hosts.current.set(selectedHostId, h);
      void listenHostEvents(h);
    }
    return h;
  }, [connection, selectedHostId, pairing, listenHostEvents]);

  const requestPairing = useCallback(async () => {
    if (!connection || !selectedHostId) throw new Error("Pick a desktop first");
    const token = pairing[selectedHostId] ?? randomPairingToken();
    setPairing((m) => rememberPairing(m, selectedHostId, token));
    const host = new HostSession(connection, selectedHostId, token);
    hosts.current.set(selectedHostId, host);
    await host.requestTrust();
  }, [connection, selectedHostId, pairing]);

  const send = useCallback(
    async (
      chatId: string,
      prompt: string,
      extras?: {
        harness?: string | null;
        model?: string | null;
        attachments?: StagedAttachment[];
      },
    ) => {
      const host = hostSession();
      if (!host) throw new Error("No desktop selected");
      const session = workspace.sessions.find((s) => s.chatId === chatId);
      const paths: string[] = [];
      for (const att of extras?.attachments ?? []) {
        const { path } = await host.uploadAttachment(att);
        paths.push(path);
      }
      await host.sendRun({
        chatId,
        prompt,
        harness: extras?.harness ?? session?.harness,
        model: extras?.model ?? session?.model,
        cwd: session?.cwd,
        attachments: paths,
      });
      await refresh();
    },
    [hostSession, workspace.sessions, refresh],
  );

  const stop = useCallback(
    async (chatId: string) => {
      const host = hostSession();
      if (!host) throw new Error("No desktop selected");
      await host.stop(chatId);
      await refresh();
    },
    [hostSession, refresh],
  );

  const transcript = useCallback(async (chatId: string) => {
    if (!edge.current) return [];
    return edge.current.chatTail(chatId);
  }, []);

  const createSession = useCallback(
    async (title: string, harness: string, model: string, reasoning?: string) => {
      const host = hostSession();
      if (!host) throw new Error("No desktop selected");
      const space = workspace.spaces.find((s) => s.deviceId === host.hostDeviceId);
      const chatId = crypto.randomUUID();
      await host.mutate({
        op: "createChat",
        chatId,
        title,
        spaceId: space?.id,
        deviceId: host.hostDeviceId,
        config: {
          harness,
          model,
          sandbox: "workspace-write",
          ...(reasoning ? { reasoning } : {}),
        },
      });
      await refresh();
      return chatId;
    },
    [hostSession, workspace.spaces, refresh],
  );

  const value = useMemo<AppState>(
    () => ({
      connection,
      workspace,
      selectedHostId,
      pairing,
      error,
      linking,
      restoring,
      signIn,
      signOut,
      refresh,
      selectHost: setSelectedHostId,
      hostSession,
      requestPairing,
      revokePairing: (id) => setPairing((m) => forgetPairing(m, id)),
      send,
      stop,
      transcript,
      createSession,
    }),
    [
      connection,
      workspace,
      selectedHostId,
      pairing,
      error,
      linking,
      restoring,
      signIn,
      signOut,
      refresh,
      hostSession,
      requestPairing,
      send,
      stop,
      transcript,
      createSession,
    ],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useApp(): AppState {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useApp outside provider");
  return ctx;
}
