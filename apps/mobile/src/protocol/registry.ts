import type {
  DeviceInfo,
  SessionStatus,
  SessionSummary,
  SpaceInfo,
} from "./types";

export const RowKind = {
  DEVICES: "devices",
  SPACES: "spaces",
  CHATS: "chats",
  SESSIONS: "sessions",
} as const;

export interface RegistryRow {
  kind: string;
  id: string;
  seq?: number;
  deleted?: boolean;
  fields: Record<string, unknown>;
  clocks?: Record<string, string>;
}

export interface RowsResponse {
  seq: number;
  full?: boolean;
  gcFloor?: number;
  rows: RegistryRow[];
  presence?: Record<string, number>;
}

export type RegistryStore = Map<string, Map<string, RegistryRow>>;

export function emptyStore(): RegistryStore {
  return new Map();
}

export function applyDelta(store: RegistryStore, resp: RowsResponse): void {
  if (resp.full) store.clear();
  for (const row of resp.rows) {
    let table = store.get(row.kind);
    if (!table) {
      table = new Map();
      store.set(row.kind, table);
    }
    if (row.deleted) table.delete(row.id);
    else table.set(row.id, row);
  }
}

function tableRows(store: RegistryStore, kind: string): RegistryRow[] {
  return [...(store.get(kind)?.values() ?? [])];
}

function str(fields: Record<string, unknown>, key: string): string | null {
  const v = fields[key];
  return typeof v === "string" && v.length ? v : v == null ? null : String(v);
}

function num(fields: Record<string, unknown>, key: string): number | null {
  const v = fields[key];
  return typeof v === "number" ? v : null;
}

function bool(fields: Record<string, unknown>, key: string): boolean {
  return fields[key] === true;
}

function obj(
  fields: Record<string, unknown>,
  key: string,
): Record<string, unknown> | null {
  const v = fields[key];
  return v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
}

export function devices(store: RegistryStore): DeviceInfo[] {
  return tableRows(store, RowKind.DEVICES).map((row) => {
    const f = row.fields;
    const caps = f.capabilities;
    return {
      id: str(f, "id") ?? row.id,
      name: str(f, "name") ?? "device",
      platform: str(f, "platform") ?? "",
      lastSeenAt: num(f, "lastSeenAt"),
      version: str(f, "version") ?? undefined,
      capabilities: Array.isArray(caps)
        ? caps.filter((c): c is string => typeof c === "string")
        : [],
    };
  });
}

export function spaces(store: RegistryStore): SpaceInfo[] {
  return tableRows(store, RowKind.SPACES).flatMap((row) => {
    const f = row.fields;
    const path = str(f, "path");
    if (!path) return [];
    return [
      {
        id: str(f, "id") ?? row.id,
        path,
        name: str(f, "name"),
        deviceId: str(f, "deviceId") ?? "",
      },
    ];
  });
}

function asStatus(raw: string | null): SessionStatus {
  switch (raw) {
    case "working":
    case "awaitingInput":
    case "errored":
    case "idle":
      return raw;
    default:
      return "idle";
  }
}

export function sessions(store: RegistryStore): SessionSummary[] {
  const statuses = new Map<string, RegistryRow>();
  for (const row of tableRows(store, RowKind.SESSIONS)) {
    statuses.set(str(row.fields, "chatId") ?? row.id, row);
  }
  return tableRows(store, RowKind.CHATS).map((row) => {
    const f = row.fields;
    const chatId = str(f, "id") ?? row.id;
    const config = obj(f, "config") ?? {};
    const statusRow = statuses.get(chatId)?.fields ?? {};
    return {
      chatId,
      title: str(f, "title") || "New session",
      harness: str(config, "harness"),
      model: str(config, "model"),
      reasoning: str(config, "reasoning"),
      branch: str(f, "branch"),
      cwd: str(f, "cwd"),
      spaceId: str(f, "spaceId"),
      deviceId: str(f, "deviceId"),
      archived: bool(f, "archived"),
      lastMessagePreview: str(f, "lastMessagePreview"),
      lastMessageAt: num(f, "lastMessageAt"),
      createdAt: num(f, "createdAt") ?? 0,
      lastSeenAt: num(f, "lastSeenAt"),
      status: asStatus(str(statusRow, "status")),
      statusUpdatedAt: num(statusRow, "updatedAt"),
    };
  });
}

export function nextHlc(deviceId: string, last: { ms: number; counter: number }): {
  hlc: string;
  ms: number;
  counter: number;
} {
  let { ms, counter } = last;
  const now = Date.now();
  if (now > ms) {
    ms = now;
    counter = 0;
  } else {
    counter += 1;
    if (counter > 999_999) {
      ms += 1;
      counter = 0;
    }
  }
  return {
    hlc: `${String(ms).padStart(13, "0")}-${String(counter).padStart(6, "0")}-${deviceId}`,
    ms,
    counter,
  };
}
