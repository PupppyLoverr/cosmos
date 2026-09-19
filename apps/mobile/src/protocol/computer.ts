import type { ComputerStatus, Connection, NoticeKind, PushDelivery } from "./types";

export const COMPUTER_METHODS = {
  status: "ComputerStatus",
  observe: "ComputerObserve",
  toolCall: "ComputerToolCall",
  stop: "ComputerStop",
  requestTrust: "ComputerRequestTrust",
  approveTrust: "ComputerApproveTrust",
  revokeTrust: "ComputerRevokeTrust",
  trustStatus: "ComputerTrustStatus",
  watch: "WatchComputer",
  watchLive: "WatchComputerLive",
  watchEvents: "WatchComputerEvents",
} as const;

export function pairingParams(
  conn: Connection,
  targetDeviceId: string,
  pairingToken: string,
  extra: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    ...extra,
    targetDeviceId,
    callerDeviceId: conn.deviceId,
    pairingToken,
  };
}

export function randomPairingToken(): string {
  const bytes = new Uint8Array(24);
  if (typeof crypto !== "undefined" && crypto.getRandomValues) {
    crypto.getRandomValues(bytes);
  } else {
    for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
  }
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export interface ComputerNotice {
  kind: NoticeKind;
  title: string;
  body: string;
  delivery: PushDelivery;
  at: string;
}

/** Local notifications are always attempted. Remote push reports the
 *  credential boundary instead of faking success. */
export function classifyPushDelivery(hasPushCredential: boolean): PushDelivery {
  return hasPushCredential ? "pushed" : "credential-missing";
}

export function computerOnline(status: ComputerStatus | null): boolean {
  return status?.state === "ready";
}
