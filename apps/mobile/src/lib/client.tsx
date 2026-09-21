// CosmosClient — the app's engine-facing state container.
//
// One client drives both transports. Every watch mirrors an engine stream into
// an in-memory snapshot consumed through useSyncExternalStore; no demo data.

import React, {
  createContext, useContext, useMemo, useSyncExternalStore,
} from 'react';
import { RpcClient, ConnectionStatus, TransportSpec, localSpec, relaySpec, StreamHandle } from './rpc';
import {
  Chat, Device, Session, Space, SessionCommandPayload, RunRequest,
  HarnessInfo, EngineInfo, ComputerStatus, UserInputAnswer,
} from './protocol';
import {
  StoredSession, loadSession, clearSession, freshAccessToken,
  idTokenExpiry, signInInteractive, EDGE_URL,
} from './auth';
import { Platform } from 'react-native';
// SecureStore has no sync API at module scope; the store below hydrates async.
import * as SecureStore from 'expo-secure-store';
import * as Crypto from 'expo-crypto';

export interface LinkedDesktop {
  /** Engine device id (remote identity) or host:port label (local pairing). */
  id: string;
  /** Display name — device name for remote, host alias for local. */
  name: string;
  kind: 'local' | 'remote';
  /** Local: `host:port`. Remote: edge device id. */
  address: string;
  trusted: boolean;
  /** Remote only: the grant token `PairPhone` returned — presented as
   *  `AuthorizePhone` on every relay conn so the desktop's pairing gate
   *  unlocks the full RPC surface for this phone. */
  deviceToken?: string;
  addedAt: number;
}

/** Edge `/registry/:orgId/rows` row: metadata lives at the root, domain
 *  values under `fields`; `deleted` rows are tombstones to skip. */
interface RegistryRow {
  kind: string;
  id: string;
  deleted?: boolean;
  fields?: Record<string, unknown>;
}

/** A device row from the account registry (remote discovery — phones that
 *  paired show up here once the desktop publishes them). */
export interface AccountDevice {
  deviceId: string;
  name: string;
  platform?: string;
  /** `GET /device/:id/status` — the desktop process is checked into the relay. */
  online?: boolean;
}

export interface ConnectionInfo {
  status: ConnectionStatus;
  detail?: string;
  desktop?: LinkedDesktop;
  localDeviceId?: string;
}

/** Name given to a local desktop until the engine's registry reports the real one. */
export const PLACEHOLDER_NAME = 'This Desktop';

/** Display name for a linked desktop: its real name, or "This Desktop" when only
 *  the address/placeholder was ever stored. */
export function desktopDisplayName(desktop: LinkedDesktop): string {
  const name = desktop.name.trim();
  return !name || name === desktop.address ? PLACEHOLDER_NAME : name;
}

const DESKTOPS_KEY = 'cosmos.desktops';
const ACTIVE_DESKTOP_KEY = 'cosmos.activeDesktop';

// Hermes has no global `crypto`; expo-crypto's randomUUID is secure RNG-backed.
const uuid = Crypto.randomUUID;

// -- snapshot store ----------------------------------------------------------

class Snapshot<T> {
  private value: T;
  private listeners = new Set<() => void>();
  constructor(initial: T) { this.value = initial; }
  get = () => this.value;
  set(v: T) {
    if (Object.is(v, this.value)) return;
    this.value = v;
    for (const fn of this.listeners) fn();
  }
  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };
}

export function useSnapshot<T>(snap: Snapshot<T>): T {
  return useSyncExternalStore(snap.subscribe, snap.get, snap.get);
}

// -- client ------------------------------------------------------------------

export class CosmosClient {
  readonly chats = new Snapshot<Chat[]>([]);
  readonly devices = new Snapshot<Device[]>([]);
  readonly sessions = new Snapshot<Session[]>([]);
  readonly spaces = new Snapshot<Space[]>([]);
  readonly connection = new Snapshot<ConnectionInfo>({ status: 'disconnected' });
  readonly auth = new Snapshot<StoredSession | null>(null);
  readonly desktops = new Snapshot<LinkedDesktop[]>([]);
  readonly engine = new Snapshot<EngineInfo | null>(null);
  readonly computer = new Snapshot<ComputerStatus | null>(null);
  readonly accessToken = new Snapshot<string | null>(null);
  /** deviceId → relay presence for linked remote desktops (edge /status probe). */
  readonly remotePresence = new Snapshot<Record<string, boolean | null>>({});

  private rpc: RpcClient | null = null;
  private streams: StreamHandle[] = [];
  private accessTokenCache: { token: string; expiresAt: number } | null = null;

  async hydrate() {
    const [session, desktopsRaw, activeRaw] = await Promise.all([
      loadSession(),
      SecureStore.getItemAsync(DESKTOPS_KEY),
      SecureStore.getItemAsync(ACTIVE_DESKTOP_KEY),
    ]);
    this.auth.set(session);
    try {
      this.desktops.set(desktopsRaw ? (JSON.parse(desktopsRaw) as LinkedDesktop[]) : []);
    } catch {
      this.desktops.set([]);
    }
    const desktops = this.desktops.get();
    const active = desktops.find((d) => d.id === activeRaw) ?? desktops[0];
    if (active) void this.connect(active);
  }

  // -- auth ------------------------------------------------------------------

  async signIn(): Promise<void> {
    const session = await signInInteractive();
    this.auth.set(session);
    await this.refreshTokenNow();
    // After sign-in, remote devices become visible through the desktops watch
    // of whichever transport is live; pairing a remote desktop needs the edge.
    const current = this.connection.get().desktop;
    if (current?.kind === 'remote') void this.connect(current);
  }

  async signOut(): Promise<void> {
    await clearSession();
    this.auth.set(null);
    this.accessToken.set(null);
    this.accessTokenCache = null;
    const current = this.connection.get().desktop;
    if (current?.kind === 'remote') this.disconnect();
  }

  private async tokenForDial(): Promise<string | null> {
    const session = this.auth.get();
    if (!session) return null;
    const cached = this.accessTokenCache;
    if (cached && cached.expiresAt > Date.now() + 30_000) return cached.token;
    if (!session.refreshToken) return null;
    try {
      const token = await freshAccessToken(session);
      this.accessTokenCache = { token: token!, expiresAt: idTokenExpiry(token!) };
      this.accessToken.set(token);
      return token;
    } catch {
      return null;
    }
  }

  private async refreshTokenNow() {
    await this.tokenForDial();
  }

  // -- desktops --------------------------------------------------------------

  private async persistDesktops() {
    await SecureStore.setItemAsync(DESKTOPS_KEY, JSON.stringify(this.desktops.get()));
  }

  async addLocalDesktop(host: string, port: number, name?: string): Promise<LinkedDesktop> {
    const address = `${host.trim()}:${port}`;
    const desktop: LinkedDesktop = {
      id: `local:${address}`,
      name: name?.trim() || address,
      kind: 'local',
      address,
      trusted: true, // user confirmed the pairing sheet
      addedAt: Date.now(),
    };
    const next = [...this.desktops.get().filter((d) => d.id !== desktop.id), desktop];
    this.desktops.set(next);
    await this.persistDesktops();
    return desktop;
  }

  async addRemoteDesktop(device: Device): Promise<LinkedDesktop> {
    const desktop: LinkedDesktop = {
      id: `remote:${device.id}`,
      name: device.name || device.id,
      kind: 'remote',
      address: device.id,
      trusted: true,
      addedAt: Date.now(),
    };
    const next = [...this.desktops.get().filter((d) => d.id !== desktop.id), desktop];
    this.desktops.set(next);
    await this.persistDesktops();
    return desktop;
  }

  /** Account-scoped fetch against the edge (registry reads, device status).
   *  The bearer is the Woozlit/Firebase id token — same one the relay
   *  WebSocket dials with. */
  private async edgeFetch(path: string): Promise<unknown | null> {
    const session = this.auth.get();
    const token = await this.tokenForDial();
    if (!session || !token) return null;
    try {
      const res = await fetch(`${EDGE_URL}${path}`, {
        headers: { authorization: `Bearer ${token}` },
      });
      if (!res.ok) return { __error: res.status };
      return await res.json();
    } catch (e) {
      return { __error: String(e) };
    }
  }

  /** Devices this account's desktops registered — the remote discovery list
   *  (replaces LAN scanning). Presence comes from `/device/:id/status`. */
  async fetchAccountDevices(): Promise<AccountDevice[]> {
    const session = this.auth.get();
    if (!session) return [];
    const rows = (await this.edgeFetch(`/registry/${encodeURIComponent(session.user.id)}/rows`)) as
      | { rows?: RegistryRow[] }
      | null;
    const devices = (rows?.rows ?? [])
      .filter((r) => r.kind === 'devices' && !r.deleted)
      .map((r) => {
        const f = (r.fields ?? {}) as Record<string, unknown>;
        const deviceId = typeof f.id === 'string' ? f.id : r.id;
        return {
          deviceId,
          name: typeof f.name === 'string' && f.name ? f.name : deviceId,
          platform: typeof f.platform === 'string' ? f.platform : undefined,
        };
      });
    // Presence probe per device — a handful of GETs, fine for a device list.
    return Promise.all(
      devices.map(async (d) => {
        const status = (await this.edgeFetch(`/device/${encodeURIComponent(d.deviceId)}/status`)) as
          | { hostConnected?: boolean }
          | null;
        return { ...d, online: status?.hostConnected === true };
      }),
    );
  }

  /** Probe relay presence for every linked remote desktop — powers the
   *  "online whenever the desktop is checked into the relay" list state. */
  async refreshRemotePresence(): Promise<void> {
    if (!this.auth.get()) { this.remotePresence.set({}); return; }
    const map: Record<string, boolean | null> = {};
    for (const d of this.desktops.get().filter((x) => x.kind === 'remote')) {
      map[d.address] = await this.deviceOnline(d.address);
    }
    this.remotePresence.set(map);
  }

  /** `GET /device/:id/status` — is the desktop process checked into the
   *  relay right now. null when signed out or the probe failed. */
  async deviceOnline(deviceId: string): Promise<boolean | null> {
    const status = (await this.edgeFetch(`/device/${encodeURIComponent(deviceId)}/status`)) as
      | { hostConnected?: boolean }
      | { __error: unknown }
      | null;
    if (!status || '__error' in status) return null;
    return status.hostConnected === true;
  }

  /** Resolve a typed 6-char pair code → the desktop's device id via the
   *  `phonePair/<code>` registry row the desktop published. */
  async resolvePairCode(code: string): Promise<{ deviceId: string } | null> {
    const session = this.auth.get();
    if (!session) return null;
    const rows = (await this.edgeFetch(`/registry/${encodeURIComponent(session.user.id)}/rows`)) as
      | { rows?: RegistryRow[] }
      | null;
    const row = (rows?.rows ?? []).find(
      (r) =>
        r.kind === 'phonePair' &&
        !r.deleted &&
        r.id.toUpperCase() === code.trim().toUpperCase(),
    );
    const f = (row?.fields ?? {}) as { deviceId?: unknown; exp?: unknown };
    const deviceId = typeof f.deviceId === 'string' ? f.deviceId : null;
    if (!deviceId) return null;
    if (typeof f.exp === 'number' && f.exp < Date.now()) return null; // expired offer
    return { deviceId };
  }

  /** The QR/typed-code redemption: dial the device room, `PairPhone` the
   *  code on this conn, store the returned grant token, then drop — the
   *  normal `connect()` re-dials and authorizes with the token. */
  async pairRemote(deviceId: string, code: string, label?: string): Promise<LinkedDesktop> {
    const rpc = new RpcClient(relaySpec(EDGE_URL, deviceId), () => this.tokenForDial());
    try {
      await new Promise<void>((resolve, reject) => {
        const off = rpc.onStatusChange((s, detail) => {
          if (s === 'connected') { off(); resolve(); }
          else if (s === 'error') { off(); reject(new Error(detail ?? 'connection failed')); }
        });
        rpc.connect();
        setTimeout(() => { off(); reject(new Error('pairing timed out')); }, 15_000);
      });
      const reply = await rpc.call<{ deviceToken: string; grantId: string; deviceId: string }>(
        'PairPhone',
        { code: code.trim().toUpperCase(), label: label ?? 'iPhone' },
      );
      const desktop: LinkedDesktop = {
        id: `remote:${reply.deviceId}`,
        name: reply.deviceId,
        kind: 'remote',
        address: reply.deviceId,
        trusted: true,
        deviceToken: reply.deviceToken,
        addedAt: Date.now(),
      };
      const next = [...this.desktops.get().filter((d) => d.id !== desktop.id), desktop];
      this.desktops.set(next);
      await this.persistDesktops();
      return desktop;
    } finally {
      rpc.close();
    }
  }

  async removeDesktop(id: string): Promise<void> {
    const next = this.desktops.get().filter((d) => d.id !== id);
    this.desktops.set(next);
    await this.persistDesktops();
    if (this.connection.get().desktop?.id === id) this.disconnect();
  }

  async renameDesktop(id: string, name: string): Promise<void> {
    const next = this.desktops.get().map((d) => (d.id === id ? { ...d, name } : d));
    this.desktops.set(next);
    await this.persistDesktops();
    const conn = this.connection.get();
    if (conn.desktop?.id === id) this.connection.set({ ...conn, desktop: { ...conn.desktop, name } });
  }

  // -- connection ------------------------------------------------------------

  disconnect() {
    for (const s of this.streams) s.cancel();
    this.streams = [];
    this.rpc?.close();
    this.rpc = null;
    this.connection.set({ ...this.connection.get(), status: 'disconnected' });
  }

  async connect(desktop: LinkedDesktop): Promise<void> {
    this.disconnect();
    let spec: TransportSpec;
    if (desktop.kind === 'local') {
      const [host, portStr] = desktop.address.split(':');
      spec = localSpec(host, Number(portStr) || 27654);
    } else {
      spec = relaySpec('https://edge.zeron.sh', desktop.address);
    }
    const rpc = new RpcClient(spec, desktop.kind === 'remote' ? () => this.tokenForDial() : undefined);
    this.rpc = rpc;
    this.connection.set({ status: 'connecting', desktop });
    rpc.onStatusChange((s, detail) => {
      this.connection.set({ status: s, detail, desktop });
      if (s === 'connected') {
        void this.afterConnect(desktop);
      }
    });
    rpc.connect();
    await SecureStore.setItemAsync(ACTIVE_DESKTOP_KEY, desktop.id);
  }

  private async afterConnect(desktop: LinkedDesktop) {
    try {
      // Remote conns land behind the pairing gate — authorize with the
      // stored grant BEFORE anything else or every call errors.
      if (desktop.kind === 'remote' && desktop.deviceToken) {
        await this.rpc!.call('AuthorizePhone', { deviceToken: desktop.deviceToken });
      }
      const [info, local] = await Promise.all([
        this.rpc!.call<EngineInfo>('EngineInfo').catch(() => null),
        this.rpc!.call<{ deviceId: string }>('LocalDevice').catch(() => null),
      ]);
      this.engine.set(info);
      if (local?.deviceId) {
        this.connection.set({ ...this.connection.get(), localDeviceId: local.deviceId });
      }
      void this.rpc!.call<ComputerStatus>('ComputerStatus')
        .then((s) => this.computer.set(s))
        .catch(() => this.computer.set(null));
      this.startWatches();
    } catch {
      // leave watches off; status already reflects the socket
    }
  }

  private startWatches() {
    for (const s of this.streams) s.cancel();
    this.streams = [];
    const wire = <T,>(method: string, snap: Snapshot<T[]>, after?: () => void) => {
      const h = this.rpc!.stream(method, {}, (item) => {
        const rows = (item as { entries?: T[] } | T[]) ?? [];
        snap.set(Array.isArray(rows) ? rows : (rows.entries ?? []));
        after?.();
      });
      h.done.catch(() => {});
      this.streams.push(h);
    };
    wire<Chat>('WatchChats', this.chats);
    wire<Device>('WatchDevices', this.devices, () => this.adoptRegistryName());
    wire<Session>('WatchSessions', this.sessions);
    wire<Space>('WatchSpaces', this.spaces);
  }



  /** The engine registers itself under the computer's real name; the linked
   *  desktop keeps whatever the user typed at pairing, so adopt the registry
   *  name when the stored one is just the address / device-id placeholder. */
  private adoptRegistryName() {
    const conn = this.connection.get();
    const desktop = conn.desktop;
    if (!desktop || !conn.localDeviceId) return;
    if (desktop.kind === 'remote' && conn.localDeviceId !== desktop.address) return;
    const real = this.devices.get().find((d) => d.id === conn.localDeviceId)?.name?.trim();
    if (!real || real === desktop.name) return;
    const placeholder = desktop.name === desktop.address || desktop.name === PLACEHOLDER_NAME || conn.localDeviceId.startsWith(desktop.name);
    if (!placeholder) return;
    void this.renameDesktop(desktop.id, real);
  }

  /** Direct RPC escape hatch (screens use typed helpers below). */
  raw(): RpcClient | null {
    return this.rpc;
  }

  // -- chat ops ----------------------------------------------------------------

  async createChat(spaceId: string | null, deviceId: string | null, config?: Partial<Chat['config']>): Promise<string> {
    const chatId = uuid();
    // The engine requires a host anchor: spaceId when a project is chosen,
    // otherwise the connected device's id (project-less chat lands on it).
    const host = deviceId ?? this.connection.get().localDeviceId ?? undefined;
    await this.rpc!.call('Mutate', {
      op: 'createChat',
      chatId,
      spaceId: spaceId ?? undefined,
      deviceId: host,
      ...(config ? { config } : {}),
    });
    return chatId;
  }

  async renameChat(chatId: string, title: string) {
    await this.rpc!.call('Mutate', { op: 'renameChat', chatId, title });
  }

  async setChatArchived(chatId: string, archived: boolean) {
    await this.rpc!.call('Mutate', { op: 'setChatArchived', chatId, archived });
  }

  async deleteChat(chatId: string) {
    await this.rpc!.call('Mutate', { op: 'deleteChat', chatId });
  }

  async setChatConfig(chatId: string, config: Chat['config']) {
    await this.rpc!.call('Mutate', { op: 'setChatConfig', chatId, config });
  }

  async markChatSeen(chatId: string) {
    await this.rpc!.call('Mutate', { op: 'markChatSeen', chatId }).catch(() => {});
  }

  // -- commands ----------------------------------------------------------------

  async queueCommand(chatId: string, command: SessionCommandPayload): Promise<void> {
    await this.rpc!.call('QueueCommand', { chatId, command });
  }

  async run(chatId: string, request: RunRequest) {
    await this.queueCommand(chatId, { kind: 'run', request, messageId: uuid() });
  }

  async steer(chatId: string, prompt: string) {
    await this.queueCommand(chatId, { kind: 'steer', prompt, messageId: uuid() });
  }

  async interrupt(chatId: string) {
    await this.queueCommand(chatId, { kind: 'interrupt' });
  }

  async respondInput(chatId: string, requestId: string, answers: UserInputAnswer[]) {
    await this.queueCommand(chatId, { kind: 'respondInput', requestId, answers });
  }

  // -- reads -------------------------------------------------------------------

  async listHarnesses(): Promise<HarnessInfo[]> {
    const res = await this.rpc!.call<{ harnesses?: HarnessInfo[] } | HarnessInfo[]>('ListHarnesses');
    return Array.isArray(res) ? res : (res.harnesses ?? []);
  }

  async listModels(harness: string): Promise<{ id: string; name?: string }[]> {
    const res = await this.rpc!.call<{ models?: { id: string; name?: string }[] } | { id: string; name?: string }[]>(
      'ListModels', { harness });
    return Array.isArray(res) ? res : (res.models ?? []);
  }

  async chatContext(chatId: string, maxRows = 24): Promise<unknown> {
    return this.rpc!.call('ChatContext', { chatId, maxRows });
  }

  async uploadAttachment(fileName: string, base64Chunks: string[]): Promise<string> {
    const uploadId = uuid();
    for (let i = 0; i < base64Chunks.length; i++) {
      await this.rpc!.call('UploadChunk', { uploadId, data: base64Chunks[i], seq: i });
    }
    const res = await this.rpc!.call<{ path: string }>('UploadCommit', { uploadId, fileName });
    return res.path;
  }

  /** Single computer screenshot for Watch Live (JPEG/PNG base64 → data uri). */
  async computerObserve(): Promise<string | null> {
    const res = await this.rpc!.call<Record<string, unknown>>('ComputerObserve', { scope: 'desktop', includeScreenshot: true });
    // The engine stages the PNG under the uploads jail and returns its path —
    // assemble it back through ReadAttachmentChunk (45KB base64 pages).
    const staged = res?.screenshot as { path?: string } | undefined;
    const path = staged?.path;
    if (typeof path !== 'string' || path.length === 0) return null;
    let offset = 0;
    let data = '';
    let mime = 'image/png';
    for (let i = 0; i < 200; i++) {
      const chunk = await this.rpc!.call<{ data: string; nextOffset: number; done: boolean; mimeType?: string }>(
        'ReadAttachmentChunk', { path, offset },
      );
      data += chunk.data;
      if (chunk.mimeType) mime = chunk.mimeType;
      if (chunk.done) return `data:${mime};base64,${data}`;
      offset = chunk.nextOffset;
    }
    return null;
  }

  async computerStatus(): Promise<ComputerStatus | null> {
    try {
      const s = await this.rpc!.call<ComputerStatus>('ComputerStatus');
      this.computer.set(s);
      return s;
    } catch {
      return null;
    }
  }
}

// -- react bindings ------------------------------------------------------------

const ClientContext = createContext<CosmosClient | null>(null);

export function CosmosProvider({ children }: { children: React.ReactNode }) {
  const client = useMemo(() => {
    const c = new CosmosClient();
    void c.hydrate();
    return c;
  }, []);
  return <ClientContext.Provider value={client}>{children}</ClientContext.Provider>;
}

export function useCosmos(): CosmosClient {
  const c = useContext(ClientContext);
  if (!c) throw new Error('CosmosProvider missing');
  return c;
}

export const THIS_PLATFORM = Platform.OS;
