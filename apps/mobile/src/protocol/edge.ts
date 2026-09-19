import { currentBearer } from "./auth";
import {
  applyDelta,
  devices,
  emptyStore,
  nextHlc,
  sessions,
  spaces,
  type RegistryStore,
  type RowsResponse,
} from "./registry";
import type {
  Connection,
  DeviceInfo,
  SessionSummary,
  SpaceInfo,
  TranscriptItem,
} from "./types";

export class CosmosError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CosmosError";
  }
}

export class EdgeClient {
  conn: Connection;
  store: RegistryStore = emptyStore();
  cursor = 0;
  presence: Record<string, number> = {};
  private hlc = { ms: 0, counter: 0 };

  constructor(conn: Connection) {
    this.conn = conn;
  }

  private get base(): string {
    return this.conn.edgeUrl.replace(/\/$/, "");
  }

  private async authed(path: string, init: RequestInit = {}): Promise<Response> {
    const token = await currentBearer(this.conn);
    const headers = new Headers(init.headers);
    headers.set("Authorization", `Bearer ${token}`);
    return fetch(`${this.base}${path}`, { ...init, headers });
  }

  async health(): Promise<string> {
    const r = await fetch(`${this.base}/health`);
    if (!r.ok) throw new CosmosError(`health ${r.status}`);
    return r.text();
  }

  async pull(): Promise<void> {
    const since = this.cursor > 0 ? `&since=${this.cursor}` : "";
    const r = await this.authed(
      `/registry/${this.conn.orgId}/rows?device=${this.conn.deviceId}&beat=1${since}`,
    );
    const body = await r.text();
    if (!r.ok) throw new CosmosError(`rows ${r.status}: ${body}`);
    const resp = JSON.parse(body) as RowsResponse;
    applyDelta(this.store, resp);
    this.cursor = resp.seq;
    this.presence = resp.presence ?? {};
  }

  devices(): DeviceInfo[] {
    return devices(this.store);
  }
  spaces(): SpaceInfo[] {
    return spaces(this.store);
  }
  sessions(): SessionSummary[] {
    return sessions(this.store).filter((s) => !s.archived);
  }

  async markSeen(chatId: string): Promise<void> {
    const clock = nextHlc(this.conn.deviceId, this.hlc);
    this.hlc = clock;
    const batch = `b-${clock.hlc}`;
    const payload = {
      batch,
      ops: [
        {
          kind: "chats",
          id: chatId,
          op: "update",
          set: { lastSeenAt: Date.now() },
          hlc: clock.hlc,
        },
      ],
    };
    const r = await this.authed(
      `/registry/${this.conn.orgId}/push?device=${this.conn.deviceId}`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(payload),
      },
    );
    if (!r.ok) throw new CosmosError(`push ${r.status}`);
  }

  async nudge(deviceId: string, chatId: string): Promise<void> {
    const r = await this.authed(`/device/${deviceId}/nudge`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ chatId }),
    });
    if (!r.ok && (r.status < 400 || r.status >= 500)) {
      throw new CosmosError(`nudge ${r.status}`);
    }
  }

  async chatTail(chatId: string): Promise<TranscriptItem[]> {
    const r = await this.authed(`/chat2/${chatId}/tail`);
    if (r.status === 404) return [];
    const body = await r.text();
    if (!r.ok) throw new CosmosError(`tail ${r.status}: ${body}`);
    if (!body.trim()) return [];
    const parsed = JSON.parse(body) as {
      messages?: Array<{
        id?: string;
        role?: string;
        createdAt?: number;
        parts?: Array<{ kind?: string; text?: string }>;
      }>;
    };
    return (parsed.messages ?? []).map((m) => ({
      id: m.id ?? "",
      role: m.role === "user" ? "user" : m.role === "system" ? "system" : "assistant",
      text: (m.parts ?? [])
        .filter((p) => p.kind === "text" && p.text)
        .map((p) => p.text as string)
        .join("\n"),
      ts: m.createdAt ?? 0,
    }));
  }
}
