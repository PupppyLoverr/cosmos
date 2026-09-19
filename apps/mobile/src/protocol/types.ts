/** Shared Cosmos companion types. No mock sessions — these mirror registry + RPC. */

export type AuthMode = "woozlit" | "dev";

export type SessionStatus = "idle" | "working" | "awaitingInput" | "errored" | "unknown";

export type ChatIndicator =
  | "awaitingInput"
  | "errored"
  | "working"
  | "completed"
  | "idle";

export type TrustState = "none" | "pending" | "trusted" | "revoked";

export type NoticeKind =
  | "task-complete"
  | "approval-required"
  | "foreground-escalation"
  | "failure"
  | "offline"
  | "reconnect"
  | "stopped";

export type PushDelivery = "local-only" | "pushed" | "credential-missing";

export interface Connection {
  edgeUrl: string;
  userId: string;
  orgId: string;
  token: string;
  refreshToken?: string;
  deviceId: string;
  deviceName: string;
  mode: AuthMode;
}

export interface DeviceInfo {
  id: string;
  name: string;
  platform: string;
  lastSeenAt: number | null;
  version?: string;
  capabilities: string[];
}

export interface SpaceInfo {
  id: string;
  path: string;
  name: string | null;
  deviceId: string;
}

export interface SessionSummary {
  chatId: string;
  title: string;
  harness: string | null;
  model: string | null;
  reasoning: string | null;
  branch: string | null;
  cwd: string | null;
  spaceId: string | null;
  deviceId: string | null;
  archived: boolean;
  lastMessagePreview: string | null;
  lastMessageAt: number | null;
  createdAt: number;
  lastSeenAt: number | null;
  status: SessionStatus;
  statusUpdatedAt: number | null;
}

export interface TranscriptItem {
  id: string;
  role: "user" | "assistant" | "system";
  text: string;
  ts: number;
  streaming?: boolean;
}

export interface ComputerStatus {
  state: string;
  platform: string;
  session?: string;
  driverVersion?: string;
  capabilities?: {
    semantic?: boolean;
    screenshots?: boolean;
    background?: boolean;
    foreground?: boolean;
    liveWatch?: boolean;
    remoteControl?: boolean;
    waylandBackgroundLimited?: boolean;
    x11InputFallback?: boolean;
    portalCapture?: boolean;
    windowTargeting?: boolean;
  };
  pendingTrust?: TrustPublic[];
  trustedDevices?: TrustPublic[];
}

export interface TrustPublic {
  deviceId: string;
  deviceName: string;
  platform?: string;
  state: TrustState;
  requestedAt: string;
  approvedAt?: string;
}

export interface StagedAttachment {
  name: string;
  mime: string;
  bytes: Uint8Array;
  source: "camera" | "gallery" | "file" | "clipboard" | "share";
}

export function sessionIndicator(s: SessionSummary, now: number): ChatIndicator {
  const stale =
    s.statusUpdatedAt != null && now - s.statusUpdatedAt > 45_000;
  const live =
    stale && (s.status === "working" || s.status === "awaitingInput")
      ? "unknown"
      : s.status;
  const unseen =
    s.lastMessageAt != null &&
    (s.lastSeenAt == null || s.lastMessageAt > s.lastSeenAt);
  if (live === "working") return "working";
  if (live === "awaitingInput") return "awaitingInput";
  if (live === "errored") return unseen ? "errored" : "idle";
  return unseen ? "completed" : "idle";
}

export function deviceOnline(
  d: DeviceInfo,
  now: number,
  presence: Record<string, number>,
): boolean {
  const beat = presence[d.id] ?? d.lastSeenAt;
  if (beat == null) return false;
  return now - beat < 45_000;
}

export function computerCapable(d: DeviceInfo): boolean {
  return (
    d.capabilities.some((c) => c.toLowerCase().includes("computer")) ||
    ["linux", "macos", "darwin", "windows"].includes(d.platform.toLowerCase())
  );
}

export function formatLastSeen(ts: number | null, now: number): string {
  if (ts == null) return "never";
  const delta = now - ts;
  if (delta < 45_000) return "just now";
  if (delta < 3_600_000) return `${Math.max(1, Math.round(delta / 60_000))}m ago`;
  if (delta < 86_400_000) return `${Math.max(1, Math.round(delta / 3_600_000))}h ago`;
  return new Date(ts).toISOString().slice(0, 16).replace("T", " ") + " UTC";
}

export function trustLabel(opts: {
  hasToken: boolean;
  pending: boolean;
  trusted: boolean;
}): TrustState {
  if (opts.trusted) return "trusted";
  if (opts.pending || opts.hasToken) return "pending";
  return "none";
}
