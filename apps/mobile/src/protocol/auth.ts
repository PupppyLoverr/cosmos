import type { AuthMode, Connection } from "./types";

/** Woozlit Firebase API key — same public web key iOS/desktop use. */
export const FIREBASE_API_KEY = "AIzaSyBbJtkBoJrKM2hjW2H3o9pgtcyiGge9yOo";
export const DEFAULT_EDGE = "https://edge.zeron.sh";

function decodeB64Url(seg: string): string {
  const padded = seg.replace(/-/g, "+").replace(/_/g, "/");
  if (typeof atob === "function") {
    try {
      return atob(padded);
    } catch {
      /* fall through */
    }
  }
  return Buffer.from(padded, "base64").toString("utf8");
}

export function jwtPayload(token: string): Record<string, unknown> | null {
  const parts = token.split(".");
  if (parts.length < 2) return null;
  try {
    return JSON.parse(decodeB64Url(parts[1]));
  } catch {
    return null;
  }
}

export function jwtSub(token: string): string | null {
  const p = jwtPayload(token);
  return typeof p?.sub === "string" ? p.sub : null;
}

export function isExpiredJwt(token: string, skewSec = 30): boolean {
  const p = jwtPayload(token);
  const exp = typeof p?.exp === "number" ? p.exp : null;
  if (exp == null) return false;
  return Date.now() / 1000 > exp - skewSec;
}

export function isFirebaseIdToken(token: string): boolean {
  const p = jwtPayload(token);
  return typeof p?.iss === "string" && p.iss.includes("securetoken.google.com");
}

export async function exchangeWoozlitCustomToken(
  customToken: string,
): Promise<{ idToken: string; refreshToken: string; localId?: string }> {
  const res = await fetch(
    `https://identitytoolkit.googleapis.com/v1/accounts:signInWithCustomToken?key=${FIREBASE_API_KEY}`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ token: customToken, returnSecureToken: true }),
    },
  );
  const body = await res.text();
  if (!res.ok) throw new Error(`Woozlit exchange failed (${res.status})`);
  return JSON.parse(body);
}

export async function refreshWoozlit(
  refreshToken: string,
): Promise<{ idToken: string; refreshToken: string }> {
  const res = await fetch(
    `https://securetoken.googleapis.com/v1/token?key=${FIREBASE_API_KEY}`,
    {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: `grant_type=refresh_token&refresh_token=${encodeURIComponent(refreshToken)}`,
    },
  );
  if (!res.ok) throw new Error(`Woozlit refresh failed (${res.status})`);
  const json = (await res.json()) as {
    id_token: string;
    refresh_token: string;
  };
  return { idToken: json.id_token, refreshToken: json.refresh_token };
}

export function devBearer(userId: string, orgId: string): string {
  return `${userId}@${orgId}`;
}

export function newDeviceId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `android-${Date.now().toString(16)}`;
}

export async function currentBearer(conn: Connection): Promise<string> {
  if (conn.mode === "dev") return conn.token;
  if (!isExpiredJwt(conn.token)) return conn.token;
  if (!conn.refreshToken) return conn.token;
  const refreshed = await refreshWoozlit(conn.refreshToken);
  conn.token = refreshed.idToken;
  conn.refreshToken = refreshed.refreshToken;
  return refreshed.idToken;
}

export function authModeFromInput(raw: string): AuthMode {
  return raw === "dev" ? "dev" : "woozlit";
}
