// One vocabulary for the phone↔desktop link, shared by every screen.

import { DESKTOP_OFFLINE, type ConnectionStatus } from './rpc';

export const PAIRING_EXPIRED = 'pairing_expired';
export const PHONE_UNPAIRED = 'phone_unpaired';

export interface LinkInput {
  status: ConnectionStatus;
  detail?: string;
}

export type LinkTone = 'online' | 'pending' | 'asleep' | 'attention' | 'offline';

export interface LinkState {
  tone: LinkTone;
  /** Short label for badges and rows ("Online", "Asleep"). */
  label: string;
  /** Sentence-length headline for notices and empty states. */
  title: string;
  body?: string;
  /** Only a fresh QR pairing recovers from this state. */
  needsPair: boolean;
}

export function describeLink(conn: LinkInput, desktopName = 'your desktop'): LinkState {
  const { status, detail } = conn;
  if (status === 'connected') {
    return { tone: 'online', label: 'Online', title: `Connected to ${desktopName}`, needsPair: false };
  }
  if (detail === PAIRING_EXPIRED) {
    return {
      tone: 'attention',
      label: 'Pairing expired',
      title: 'Pair this phone again',
      body: 'Pairings last six months. Show a fresh code on the desktop to reconnect.',
      needsPair: true,
    };
  }
  if (detail === PHONE_UNPAIRED) {
    return {
      tone: 'attention',
      label: 'Unpaired',
      title: 'This phone was unpaired',
      body: 'It was removed on the desktop. Pair again with a fresh code.',
      needsPair: true,
    };
  }
  if (detail === DESKTOP_OFFLINE) {
    return {
      tone: 'asleep',
      label: 'Asleep',
      title: `${desktopName} is asleep or closed`,
      body: 'Still paired. Cosmos reconnects on its own as soon as it wakes up.',
      needsPair: false,
    };
  }
  if (status === 'connecting' || status === 'reconnecting') {
    return { tone: 'pending', label: 'Connecting…', title: `Connecting to ${desktopName}…`, needsPair: false };
  }
  if (status === 'error' && detail === 'signed out') {
    return {
      tone: 'pending',
      label: 'Waiting for network',
      title: 'Waiting for a connection',
      body: 'Couldn\u2019t refresh your sign-in yet. Retrying automatically.',
      needsPair: false,
    };
  }
  if (status === 'disconnected') {
    return { tone: 'offline', label: 'Offline', title: `Not connected to ${desktopName}`, needsPair: false };
  }
  return { tone: 'attention', label: 'Unavailable', title: `Can\u2019t reach ${desktopName}`, body: detail, needsPair: false };
}
