// First-run gate for the onboarding flow. SecureStore-backed so a delete
// and reinstall re-shows it, but a settings change does not.

import * as SecureStore from 'expo-secure-store';
import { loadSession } from './auth';

const KEY = 'cosmos.onboarding.seen';
/** Mirrors `DESKTOPS_KEY` in client.tsx (read-only here). */
const DESKTOPS_KEY = 'cosmos.desktops';

let cached: boolean | null = null;

const listeners = new Set<() => void>();

export function subscribeOnboardingGate(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export async function hasSeenOnboarding(): Promise<boolean> {
  if (cached !== null) return cached;
  try {
    const v = await SecureStore.getItemAsync(KEY);
    cached = v === '1';
  } catch {
    cached = false;
  }
  return cached;
}

/**
 * First launch shows the four-beat welcome — unless this phone is already
 * set up (signed in with a desktop linked), which marks it seen silently.
 * Reads the same persisted records `CosmosClient.hydrate` restores.
 */
export async function shouldShowOnboarding(): Promise<boolean> {
  if (await hasSeenOnboarding()) return false;
  const [session, desktopsRaw] = await Promise.all([
    loadSession().catch(() => null),
    SecureStore.getItemAsync(DESKTOPS_KEY).catch(() => null),
  ]);
  let linked = false;
  try {
    const list: unknown = JSON.parse(desktopsRaw ?? '[]');
    linked = Array.isArray(list) && list.length > 0;
  } catch { /* ignore */ }
  if (session && linked) {
    await markOnboardingSeen();
    return false;
  }
  return true;
}

export async function markOnboardingSeen(): Promise<void> {
  cached = true;
  try { await SecureStore.setItemAsync(KEY, '1'); } catch { /* ignore */ }
}

export async function markOnboardingUnseen(): Promise<void> {
  cached = false;
  for (const fn of listeners) fn();
  try { await SecureStore.deleteItemAsync(KEY); } catch { /* ignore */ }
}

export function _resetOnboardingCacheForTests() {
  cached = null;
}
