import { randomPairingToken } from "../protocol/computer";
import type { Connection, NoticeKind, PushDelivery } from "../protocol/types";

export interface Notice {
  kind: NoticeKind;
  title: string;
  body: string;
  delivery: PushDelivery;
  at: number;
}

export function deliveryForPush(hasCredential: boolean): PushDelivery {
  return hasCredential ? "pushed" : "credential-missing";
}

/** Production path to the credential boundary. Never reports success without a token. */
export async function presentNotice(
  notice: Omit<Notice, "delivery" | "at">,
  opts: { hasPushCredential: boolean; presentLocal: (n: Notice) => Promise<void> },
): Promise<Notice> {
  const full: Notice = {
    ...notice,
    delivery: deliveryForPush(opts.hasPushCredential),
    at: Date.now(),
  };
  await opts.presentLocal(full);
  return full;
}

export function newPairingSecret(): string {
  return randomPairingToken();
}

export type PairingMap = Record<string, string>;

export function rememberPairing(
  map: PairingMap,
  hostDeviceId: string,
  token: string,
): PairingMap {
  return { ...map, [hostDeviceId]: token };
}

export function forgetPairing(map: PairingMap, hostDeviceId: string): PairingMap {
  const next = { ...map };
  delete next[hostDeviceId];
  return next;
}

export function targetOrThrow(conn: Connection | null): Connection {
  if (!conn) throw new Error("Sign in first");
  return conn;
}
