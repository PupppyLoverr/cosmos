// "Sign in with ChatGPT" — use a Plus/Pro plan instead of an API key.
// The same PKCE flow as Codex CLI and zcli (rust-tui/crates/zcore/src/
// chatgpt.rs): browser → auth.openai.com → loopback :1455 → code exchange.
// The session is stored with zcli's in credentials.json (creds.ts), so one
// sign-in serves both CLIs; the access token is refreshed before it expires.
import { createHash, randomBytes } from "node:crypto";
import { spawn } from "node:child_process";
import http from "node:http";

import * as creds from "./creds";
import type { ChatgptTokens } from "./creds";

export const CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann"; // Codex public client
const ISSUER = "https://auth.openai.com";
const PORT = 1455;
const REDIRECT = `http://localhost:${PORT}/auth/callback`;
const SCOPE = "openid profile email offline_access";
const CLAIMS = "https://api.openai.com/auth";

const b64url = (b: Buffer) => b.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
const now = () => Math.floor(Date.now() / 1000);

export function pkce(): { verifier: string; challenge: string } {
  const verifier = b64url(randomBytes(64));
  return { verifier, challenge: b64url(createHash("sha256").update(verifier).digest()) };
}

export function authorizeUrl(challenge: string, state: string): string {
  const q = new URLSearchParams({
    response_type: "code",
    client_id: CLIENT_ID,
    redirect_uri: REDIRECT,
    scope: SCOPE,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state,
    id_token_add_organizations: "true",
    codex_cli_simplified_flow: "true",
    originator: "zdev",
  });
  return `${ISSUER}/oauth/authorize?${q}`;
}

/** Decode a JWT payload (no signature check — read only for its claims). */
export function jwtClaims(token: string): Record<string, any> | null {
  try {
    return JSON.parse(Buffer.from(token.split(".")[1], "base64url").toString("utf8"));
  } catch {
    return null;
  }
}

export function tokensFrom(v: Record<string, any>, prevRefresh?: string): ChatgptTokens {
  const access = v.access_token;
  if (!access) throw new Error("token response missing access_token");
  const refresh = v.refresh_token ?? prevRefresh;
  if (!refresh) throw new Error("token response missing refresh_token");
  const claims = jwtClaims(access);
  if (!claims) throw new Error("access token is not a JWT");
  const auth = claims[CLAIMS] ?? {};
  const account = auth.chatgpt_account_id;
  if (!account) throw new Error("no ChatGPT account on this login");
  const id = v.id_token ? jwtClaims(v.id_token) : null;
  return {
    access,
    refresh,
    expires_at: now() + (Number(v.expires_in) || 3600),
    account_id: account,
    email: id?.email ?? claims.email ?? null,
    plan: auth.chatgpt_plan_type ?? id?.[CLAIMS]?.chatgpt_plan_type ?? null,
  };
}

async function tokenRequest(form: Record<string, string>): Promise<Record<string, any>> {
  let resp: Response;
  try {
    resp = await fetch(`${ISSUER}/oauth/token`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams(form).toString(),
      signal: AbortSignal.timeout(30_000),
    });
  } catch (e: any) {
    throw new Error(`could not reach auth.openai.com: ${e?.message ?? e}`);
  }
  const text = await resp.text();
  if (!resp.ok) throw new Error(`ChatGPT sign-in failed (${resp.status}): ${text.slice(0, 300)}`);
  return JSON.parse(text);
}

export async function refresh(t: ChatgptTokens): Promise<ChatgptTokens> {
  const v = await tokenRequest({ grant_type: "refresh_token", refresh_token: t.refresh, client_id: CLIENT_ID });
  const n = tokensFrom(v, t.refresh);
  n.email ??= t.email ?? null;
  n.plan ??= t.plan ?? null;
  return n;
}

/** The signed-in session, refreshed when it expires within five minutes. */
export async function fresh(): Promise<ChatgptTokens | null> {
  const t = creds.chatgpt();
  if (!t) return null;
  if (t.expires_at > now() + 300) return t;
  try {
    const n = await refresh(t);
    creds.saveChatgpt(n);
    return n;
  } catch {
    return t; // let the request fail with a clear 401
  }
}

export function openBrowser(url: string): void {
  const [cmd, args]: [string, string[]] =
    process.platform === "win32"
      ? ["rundll32", ["url.dll,FileProtocolHandler", url]]
      : process.platform === "darwin"
        ? ["open", [url]]
        : ["xdg-open", [url]];
  try {
    spawn(cmd, args, { detached: true, stdio: "ignore", windowsHide: true }).unref();
  } catch {}
}

const PAGE =
  "<!doctype html><meta charset=utf-8><title>zdev</title><body style=\"font:16px system-ui;background:#0b0b0c;color:#eee;display:grid;place-items:center;height:100vh;margin:0\"><div><h2 style=font-weight:600>Signed in to zdev</h2><p style=color:#8a8a8a>You can close this tab and return to the terminal.</p></div>";

/**
 * Full browser sign-in. `onUrl` receives the authorize URL (show it; the
 * browser is opened too). Resolves with the saved session; rejects on
 * cancel (`signal`), error or a five-minute timeout.
 */
export function login(onUrl: (url: string) => void, signal?: AbortSignal): Promise<ChatgptTokens> {
  const p = pkce();
  const state = b64url(randomBytes(16));
  return new Promise<ChatgptTokens>((resolve, reject) => {
    let settled = false;
    const finish = (err: Error | null, t?: ChatgptTokens) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      srv.close();
      err ? reject(err) : resolve(t!);
    };
    const onAbort = () => finish(new Error("sign-in cancelled"));
    const timer = setTimeout(() => finish(new Error("sign-in timed out")), 300_000);
    const srv = http.createServer(async (req, res) => {
      const u = new URL(req.url ?? "/", `http://localhost:${PORT}`);
      if (u.pathname !== "/auth/callback") {
        res.writeHead(404).end();
        return;
      }
      const page = (body: string) => res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(body);
      if (u.searchParams.get("state") !== state) {
        page("State mismatch — start the sign-in again from zdev.");
        return finish(new Error("sign-in state mismatch"));
      }
      const err = u.searchParams.get("error");
      if (err) {
        page(`Sign-in failed: ${err}`);
        return finish(new Error(`sign-in failed: ${err}`));
      }
      const code = u.searchParams.get("code");
      if (!code) {
        page("Sign-in failed: no code");
        return finish(new Error("callback had no code"));
      }
      try {
        const v = await tokenRequest({
          grant_type: "authorization_code",
          code,
          client_id: CLIENT_ID,
          redirect_uri: REDIRECT,
          code_verifier: p.verifier,
        });
        const t = tokensFrom(v);
        creds.saveChatgpt(t);
        page(PAGE);
        finish(null, t);
      } catch (e: any) {
        page(`Sign-in failed: ${e?.message ?? e}`);
        finish(e instanceof Error ? e : new Error(String(e)));
      }
    });
    srv.on("error", (e: any) =>
      finish(new Error(e?.code === "EADDRINUSE" ? `port ${PORT} is busy — close other Codex/zcli sign-ins and retry` : String(e?.message ?? e))),
    );
    if (signal?.aborted) return onAbort();
    signal?.addEventListener("abort", onAbort);
    srv.listen(PORT, "127.0.0.1", () => {
      const url = authorizeUrl(p.challenge, state);
      onUrl(url);
      openBrowser(url);
    });
  });
}

export function logout(): void {
  creds.clearChatgpt();
}
