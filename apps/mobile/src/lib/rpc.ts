// WebSocket RPC client for the Cosmos engine.
//
// Two transports share one framing:
//   local — `ws://<host>:<port>` speaks `{id,method,params}` text frames and
//           replies `{id, ok}` | `{id, err}` | `{id, item}` | `{id, done}`.
//   relay — `wss://<edge>/device/<deviceId>/ws?role=client&token=…` carries the
//           same RPC JSON inside a uleb128-length-prefixed JSON header
//           (`{s: streamId, k: "rpc"}`) — see crates/rpc/src/device_room.rs.
//
// The server rejects handshakes that carry an Origin header (cross-origin
// defense); React Native's WebSocket sends none.

export type ConnectionStatus =
  | 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'error';

export interface StreamHandle {
  cancel(): void;
}

interface PendingCall {
  resolve: (value: unknown) => void;
  reject: (err: Error) => void;
  onItem?: (item: unknown) => void;
  timer: ReturnType<typeof setTimeout>;
}

const DEFAULT_TIMEOUT_MS = 30_000;
const STREAM_TIMEOUT_MS = 600_000;

// ---- device-room relay framing (uleb128 len ‖ {"s","k"}‖ payload) ----------

function ulebEncode(len: number): number[] {
  const out: number[] = [];
  let n = len;
  for (;;) {
    let b = n & 0x7f;
    n >>= 7;
    if (n !== 0) b |= 0x80;
    out.push(b);
    if (n === 0) break;
  }
  return out;
}

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

export function encodeRelayFrame(streamId: string, kind: string, payload: string, to?: string): ArrayBuffer {
  const header: Record<string, string> = { s: streamId, k: kind };
  if (to) header.to = to;
  const headerBytes = textEncoder.encode(JSON.stringify(header));
  const payloadBytes = textEncoder.encode(payload);
  const len = ulebEncode(headerBytes.length);
  const out = new Uint8Array(len.length + headerBytes.length + payloadBytes.length);
  out.set(len, 0);
  out.set(headerBytes, len.length);
  out.set(payloadBytes, len.length + headerBytes.length);
  return out.buffer as ArrayBuffer;
}

export function decodeRelayFrame(bytes: ArrayBuffer): { header: { s: string; k: string; to?: string; from?: string }; payload: string } | null {
  const buf = new Uint8Array(bytes);
  let offset = 0;
  let len = 0;
  let shift = 0;
  for (;;) {
    if (offset >= buf.length) return null;
    const b = buf[offset++];
    len |= (b & 0x7f) << shift;
    if ((b & 0x80) === 0) break;
    shift += 7;
    if (shift >= 32) return null;
  }
  if (offset + len > buf.length) return null;
  try {
    const header = JSON.parse(textDecoder.decode(buf.subarray(offset, offset + len)));
    return { header, payload: textDecoder.decode(buf.subarray(offset + len)) };
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------

export interface TransportSpec {
  url: string;
  relay: boolean;
}

export class RpcClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, PendingCall>();
  private statusListeners = new Set<(s: ConnectionStatus, detail?: string) => void>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelay = 1_000;
  private closed = false;
  private status: ConnectionStatus = 'disconnected';

  constructor(
    private readonly spec: TransportSpec,
    private readonly tokenProvider?: () => Promise<string | null>,
  ) {}

  onStatusChange(fn: (s: ConnectionStatus, detail?: string) => void): () => void {
    this.statusListeners.add(fn);
    fn(this.status);
    return () => this.statusListeners.delete(fn);
  }

  getStatus(): ConnectionStatus {
    return this.status;
  }

  private setStatus(s: ConnectionStatus, detail?: string) {
    this.status = s;
    for (const fn of this.statusListeners) fn(s, detail);
  }

  connect() {
    if (this.closed) return;
    void this.dial();
  }

  close() {
    this.closed = true;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.ws?.close();
    this.failAll(new Error('connection closed'));
    this.setStatus('disconnected');
  }

  private async dial() {
    if (this.closed) return;
    this.setStatus(this.status === 'disconnected' ? 'connecting' : 'reconnecting');
    let url = this.spec.url;
    if (this.spec.relay && this.tokenProvider) {
      const token = await this.tokenProvider();
      if (!token) {
        this.setStatus('error', 'signed out');
        this.scheduleReconnect();
        return;
      }
      const sep = url.includes('?') ? '&' : '?';
      url = `${url}${sep}token=${encodeURIComponent(token)}`;
    }
    let ws: WebSocket;
    try {
      ws = new WebSocket(url);
      ws.binaryType = 'arraybuffer';
    } catch (e) {
      this.setStatus('error', String(e));
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      this.reconnectDelay = 1_000;
      this.setStatus('connected');
    };
    ws.onerror = () => {
      // onclose follows; nothing else to do here.
    };
    ws.onclose = (ev) => {
      const detail = ev.reason || undefined;
      this.failAll(new Error(`socket closed (${ev.code})`));
      if (!this.closed) {
        this.setStatus('reconnecting', detail);
        this.scheduleReconnect();
      }
    };
    ws.onmessage = (ev) => {
      if (this.spec.relay) {
        const frame = typeof ev.data === 'string'
          ? decodeRelayFrame(textEncoder.encode(ev.data).buffer as ArrayBuffer)
          : decodeRelayFrame(ev.data as ArrayBuffer);
        if (!frame || frame.header.k !== 'rpc') return;
        this.handleText(frame.payload);
      } else if (typeof ev.data === 'string') {
        this.handleText(ev.data);
      }
    };
  }

  private scheduleReconnect() {
    if (this.closed) return;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = setTimeout(() => void this.dial(), this.reconnectDelay);
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, 15_000);
  }

  private failAll(err: Error) {
    const pending = [...this.pending.values()];
    this.pending.clear();
    for (const p of pending) {
      clearTimeout(p.timer);
      p.reject(err);
    }
  }

  private handleText(text: string) {
    let frame: { id?: number; ok?: unknown; err?: string | { message?: string }; item?: unknown; done?: boolean };
    try {
      frame = JSON.parse(text);
    } catch {
      return;
    }
    const id = frame.id;
    if (typeof id !== 'number') return;
    const pending = this.pending.get(id);
    if (!pending) return;
    if (frame.item !== undefined && pending.onItem) {
      pending.onItem(frame.item);
      return;
    }
    if (frame.done === true) {
      clearTimeout(pending.timer);
      this.pending.delete(id);
      pending.resolve(undefined);
      return;
    }
    if (frame.err !== undefined) {
      clearTimeout(pending.timer);
      this.pending.delete(id);
      const msg = typeof frame.err === 'string' ? frame.err : frame.err?.message ?? 'RPC error';
      pending.reject(new Error(msg));
      return;
    }
    if ('ok' in frame) {
      clearTimeout(pending.timer);
      this.pending.delete(id);
      pending.resolve(frame.ok);
    }
  }

  private send(payload: unknown) {
    const text = JSON.stringify(payload);
    if (this.spec.relay) {
      this.ws?.send(encodeRelayFrame('rpc-0', 'rpc', text));
    } else {
      this.ws?.send(text);
    }
  }

  call<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T> {
    return this.request(method, params, undefined, DEFAULT_TIMEOUT_MS) as Promise<T>;
  }

  stream(method: string, params: Record<string, unknown> | undefined, onItem: (item: unknown) => void): StreamHandle & { done: Promise<void> } {
    let cancelFn: () => void = () => {};
    const done = new Promise<void>((resolve, reject) => {
      this.request(method, params, onItem, STREAM_TIMEOUT_MS)
        .then(() => resolve())
        .catch(reject);
      const id = this.nextId - 1;
      cancelFn = () => {
        const p = this.pending.get(id);
        if (p) {
          clearTimeout(p.timer);
          this.pending.delete(id);
        }
        resolve();
      };
    });
    return { cancel: () => cancelFn(), done };
  }

  private request(
    method: string,
    params: Record<string, unknown> | undefined,
    onItem: ((item: unknown) => void) | undefined,
    timeoutMs: number,
  ): Promise<unknown> {
    return new Promise((resolve, reject) => {
      if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
        reject(new Error('not connected'));
        return;
      }
      const id = this.nextId++;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, onItem, timer });
      this.send({ id, method, params: params ?? {} });
    });
  }
}

// Build the dial spec for each transport mode.
export function localSpec(host: string, port = 27654): TransportSpec {
  const h = host.includes(':') ? `[${host}]` : host;
  return { url: `ws://${h}:${port}`, relay: false };
}

export function relaySpec(edgeUrl: string, deviceId: string): TransportSpec {
  const base = edgeUrl.replace(/^http/, 'ws').replace(/\/+$/, '');
  return { url: `${base}/device/${encodeURIComponent(deviceId)}/ws?role=client`, relay: true };
}
