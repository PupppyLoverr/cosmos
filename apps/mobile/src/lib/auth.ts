// Woozlit (Firebase) auth — mirrors crates/engine/src/auth.rs.
//
// Flow: open `https://www.woozlit.com/login?desktop=1&callback=<cb>?state=<uuid>`
// in an auth session; Woozlit redirects to `<cb>?state=<uuid>&token=<firebase
// custom or id token>&userId&email&displayName`. A custom token exchanges at
// identitytoolkit; an id token is used as-is. Refresh rides securetoken.
//
// Secrets live in expo-secure-store, never in AsyncStorage or logs.

import * as Crypto from 'expo-crypto';
import * as Linking from 'expo-linking';
import * as SecureStore from 'expo-secure-store';
import * as WebBrowser from 'expo-web-browser';

const FIREBASE_API_KEY = 'AIzaSyBbJtkBoJrKM2hjW2H3o9pgtcyiGge9yOo';
export const WOOZLIT_ORIGIN = 'https://www.woozlit.com';
export const EDGE_URL = 'https://edge.zeron.sh';
const SESSION_KEY = 'woozlit.session';
const SIGN_IN_TTL_MS = 10 * 60 * 1000;

export interface AuthUser {
  id: string;
  email: string;
  name?: string | null;
}

export interface StoredSession {
  refreshToken: string;
  /** The live Firebase id token — Woozlit may return one directly (no custom
   *  token → no refresh token), so it must be persisted or every relay call
   *  immediately fails 'signed out'. */
  accessToken?: string | null;
  accessTokenExpiresAt?: number | null;
  user: AuthUser;
}

function jwtClaims(token: string): { sub?: string; email?: string } | null {
  try {
    const parts = token.split('.');
    if (parts.length !== 3) return null;
    const b64 = parts[1].replace(/-/g, '+').replace(/_/g, '/');
    return JSON.parse(globalThis.atob(b64));
  } catch {
    return null;
  }
}

function isFirebaseIdToken(token: string): boolean {
  const claims = jwtClaims(token);
  return !!claims && token.split('.').length === 3;
}

// Hermes has no global `crypto`; expo-crypto's randomUUID is RFC4122 v4.
const uuid = Crypto.randomUUID;

// Deep link the Woozlit page redirects back to. `createURL` resolves to
// `cosmos://auth/callback` in standalone/dev-client builds and the matching
// `exp://<host>/--/auth/callback` inside Expo Go — where the `cosmos`
// scheme is not registered. WebBrowser matches on the returned URL either way.
function callbackUrl(state: string): string {
  return `${Linking.createURL('/auth/callback')}?state=${state}`;
}

export function buildSignInUrl(): { url: string; state: string; callback: string } {
  const state = uuid();
  const callback = callbackUrl(state);
  const url = `${WOOZLIT_ORIGIN}/login?desktop=1&callback=${encodeURIComponent(callback)}`;
  return { url, state, callback };
}

interface TokenBundle {
  accessToken: string;
  refreshToken: string;
  localId: string;
}

async function signInWithCustomToken(customToken: string): Promise<TokenBundle> {
  const res = await fetch(
    `https://identitytoolkit.googleapis.com/v1/accounts:signInWithCustomToken?key=${FIREBASE_API_KEY}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token: customToken, returnSecureToken: true }),
    },
  );
  if (!res.ok) {
    throw new Error(`Woozlit token exchange failed (${res.status})`);
  }
  const body = (await res.json()) as { idToken?: string; refreshToken?: string; localId?: string };
  if (!body.idToken) throw new Error('malformed Woozlit auth response');
  return { accessToken: body.idToken, refreshToken: body.refreshToken ?? '', localId: body.localId ?? '' };
}

export async function refreshAccessToken(refreshToken: string): Promise<TokenBundle> {
  // Bounded: an unstalled dial should never wait minutes for a token.
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 12_000);
  let res: Response;
  try {
    res = await fetch(
      `https://securetoken.googleapis.com/v1/token?key=${FIREBASE_API_KEY}`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: `grant_type=refresh_token&refresh_token=${encodeURIComponent(refreshToken)}`,
        signal: ctl.signal,
      },
    );
  } finally {
    clearTimeout(timer);
  }
  if (!res.ok) throw new Error(`token refresh failed (${res.status})`);
  const body = (await res.json()) as {
    id_token?: string; access_token?: string; refresh_token?: string; user_id?: string;
  };
  const accessToken = body.id_token ?? body.access_token;
  if (!accessToken) throw new Error('malformed refresh response');
  return { accessToken, refreshToken: body.refresh_token ?? refreshToken, localId: body.user_id ?? '' };
}

export function idTokenExpiry(token: string): number {
  const claims = jwtClaims(token) as { exp?: number } | null;
  return claims?.exp ? claims.exp * 1000 : 0;
}

export async function loadSession(): Promise<StoredSession | null> {
  try {
    const raw = await SecureStore.getItemAsync(SESSION_KEY);
    return raw ? (JSON.parse(raw) as StoredSession) : null;
  } catch {
    return null;
  }
}

export async function saveSession(session: StoredSession): Promise<void> {
  await SecureStore.setItemAsync(SESSION_KEY, JSON.stringify(session));
}

export async function clearSession(): Promise<void> {
  await SecureStore.deleteItemAsync(SESSION_KEY);
}

/**
 * Runs the whole sign-in: opens the Woozlit login page in an auth session,
 * waits for the cosmos:// callback, validates the state, and exchanges the
 * token. Returns the stored session (refresh token persisted).
 */
export async function signInInteractive(): Promise<StoredSession> {
  const { url, state, callback } = buildSignInUrl();
  const startedAt = Date.now();
  const result = await WebBrowser.openAuthSessionAsync(url, callback.split('?')[0]);
  if (result.type !== 'success' || !result.url) {
    throw new Error('sign-in canceled');
  }
  if (Date.now() - startedAt > SIGN_IN_TTL_MS) {
    throw new Error('sign-in expired — start again');
  }
  const params = new URLSearchParams(result.url.split('?')[1] ?? '');
  const cbState = params.get('state');
  const token = params.get('token') ?? params.get('code');
  if (!token || !cbState || cbState !== state) {
    throw new Error('invalid or expired sign-in — start again');
  }
  const userId = params.get('userId') ?? undefined;
  const email = params.get('email') ?? undefined;
  const displayName = params.get('displayName') ?? undefined;

  let accessToken: string;
  let refreshToken: string;
  let localId = '';
  if (isFirebaseIdToken(token)) {
    accessToken = token;
    refreshToken = '';
  } else {
    const bundle = await signInWithCustomToken(token);
    accessToken = bundle.accessToken;
    refreshToken = bundle.refreshToken;
    localId = bundle.localId;
  }
  const claims = jwtClaims(accessToken);
  const user: AuthUser = {
    id: userId || claims?.sub || localId || 'woozlit-user',
    email: email || claims?.email || '',
    name: displayName ?? null,
  };
  if (!refreshToken) {
    // id-token sessions can't refresh — keep the session but surface that
    // remote sockets will need a fresh sign-in when it expires.
    refreshToken = '';
  }
  const session = {
    refreshToken,
    accessToken,
    accessTokenExpiresAt: idTokenExpiry(accessToken),
    user,
  };
  await saveSession(session);
  return session;
}

export async function freshAccessToken(session: StoredSession): Promise<string | null> {
  if (!session.refreshToken) return null;
  const bundle = await refreshAccessToken(session.refreshToken);
  await saveSession({
    refreshToken: bundle.refreshToken,
    accessToken: bundle.accessToken,
    accessTokenExpiresAt: idTokenExpiry(bundle.accessToken),
    user: session.user,
  });
  return bundle.accessToken;
}
