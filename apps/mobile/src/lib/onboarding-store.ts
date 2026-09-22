// First-run gate for the onboarding flow. SecureStore-backed so a delete
// and reinstall re-shows it, but a settings change does not.

import * as SecureStore from 'expo-secure-store';

const KEY = 'cosmos.onboarding.seen';

let cached: boolean | null = null;

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

export async function markOnboardingSeen(): Promise<void> {
  cached = true;
  try { await SecureStore.setItemAsync(KEY, '1'); } catch { /* ignore */ }
}

export async function markOnboardingUnseen(): Promise<void> {
  cached = false;
  try { await SecureStore.deleteItemAsync(KEY); } catch { /* ignore */ }
}

export function _resetOnboardingCacheForTests() {
  cached = null;
}
