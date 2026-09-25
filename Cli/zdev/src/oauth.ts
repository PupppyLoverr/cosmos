// ChatGPT subscription login — the same PKCE flow Codex CLI uses:
// browser → auth.openai.com → loopback :1455 → code exchange.
// Tokens live in ~/.cosmos-cli/openai-auth.json (never in config.toml,
// never on argv). The chatgpt provider kind refreshes on 401/expiry.
import { createHash, randomBytes } from "node:crypto";
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann"; // Codex public client
const ISSUER = "https://auth.openai.com";
const REDIRECT_PATH = "/auth/callback";
const PORT = 1455;
const SCOPES = "openid profile email offline_access";

export type AuthFile = {
  access_token: string;
  refresh_token?: string;
  expires_at?: number; // epoch ms
  account_id?: string; // chatgpt account (JWT claim)
  email?: string;
};

const b64url = (b: Buffer) =>
  b.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

export function authPath(home: string): string {
  return path.join(home, "openai-auth.json");
}

export function loadAuth(home: string): AuthFile | null {
  try {
    return JSON.parse(fs.readFileSync(authPath(home), "utf8"));
  } catch {
    return null;
  }
}

export function saveAuth(home: string, a: AuthFile): void {
  fs.mkdirSync(home, { recursive: true });
  fs.writeFileSync(authPath(home), JSON.stringify(a, null, 2), "utf8");
  try {
    fs.chmodSync(authPath(home), 0o600);
  } catch {}
}

export function clearAuth(home: string): void {
  try {
    fs.unlinkSync(authPath(home));
  } catch {}
}

function accountFromJwt(jwt: string): { account?: string; email?: string } {
  try {
    const payload = JSON.parse(
      Buffer.from(jwt.split(".")[1], "base64url").toString("utf8"),
    );
    return {
      account:
        payload["chatgpt_account_id"] ??
        payload["https://api.openai.com/auth"]?.chatgpt_account_id,
      email: payload.email,
    };
  } catch {
    return {};
  }
}

async function tokenRequest(
  body: Record<string, string>,
): Promise<Record<string, any>> {
  const resp = await fetch(`${ISSUER}/oauth/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams(body).toString(),
    signal: AbortSignal.timeout(30_000),
  });
  const v = (await resp.json().catch(() => ({}))) as Record<string, any>;
  if (!resp.ok)
    throw new Error(String(v?.error_description ?? `token ${resp.status}`));
  return v;
}

/** Refresh if the access token is expired (or within 60s of it). */
export async function accessToken(home: string): Promise<AuthFile> {
  const a = loadAuth(home);
  if (!a) throw new Error("not signed in — run `zdev login`");
  if (a.expires_at && a.expires_at - 60_000 > Date.now()) return a;
  if (!a.refresh_token) return a; // no refresh path: use what we have
  const v = await tokenRequest({
    grant_type: "refresh_token",
    refresh_token: a.refresh_token,
    client_id: CLIENT_ID,
  });
  const next: AuthFile = {
    access_token: v.access_token ?? a.access_token,
    refresh_token: v.refresh_token ?? a.refresh_token,
    expires_at: v.expires_in
      ? Date.now() + v.expires_in * 1000
      : a.expires_at,
    account_id: a.account_id,
    email: a.email,
  };
  saveAuth(home, next);
  return next;
}

/**
 * Interactive login. Opens the system browser at the authorize URL and
 * listens on http://localhost:1455/auth/callback for the redirect.
 * `onOpen` lets the caller surface the URL (for terminals where `start`
 * fails, the user can paste it).
 */
export async function loginChatGPT(
  home: string,
  onOpen?: (url: string) => void,
): Promise<AuthFile> {
  const verifier = b64url(randomBytes(32));
  const challenge = b64url(
    createHash("sha256").update(verifier).digest(),
  );
  const state = b64url(randomBytes(16));
  const redirect = `http://localhost:${PORT}${REDIRECT_PATH}`;
  const url =
    `${ISSUER}/oauth/authorize?` +
    new URLSearchParams({
      response_type: "code",
      client_id: CLIENT_ID,
      redirect_uri: redirect,
      scope: SCOPES,
      code_challenge: challenge,
      code_challenge_method: "S256",
      state,
      id_token_add_organizations: "true",
      codex_cli_simplified_flow: "true",
    }).toString();

  const code = await new Promise<string>((resolve, reject) => {
    const srv = http.createServer((req, res) => {
      const u = new URL(req.url ?? "/", `http://localhost:${PORT}`);
      if (u.pathname !== REDIRECT_PATH) {
        res.writeHead(404).end();
        return;
      }
      if (u.searchParams.get("state") !== state) {
        res.writeHead(400).end("state mismatch");
        return;
      }
      const c = u.searchParams.get("code");
      if (!c) {
        res.writeHead(400).end("no code");
        return;
      }
      res
        .writeHead(200, { "content-type": "text/html" })
        .end(
          "<html><body style='background:#000;color:#fff;font:16px monospace;" +
            "display:flex;align-items:center;justify-content:center;height:100vh'>" +
            "signed in — you can close this tab</body></html>",
        );
      srv.close();
      resolve(c);
    });
    srv.on("error", reject);
    srv.listen(PORT, "127.0.0.1", () => {
      onOpen?.(url);
      // best-effort browser open (Windows start / macOS open / xdg-open)
      const opener: [string, string[]] =
        process.platform === "win32"
          ? ["cmd", ["/c", "start", "", url]]
          : process.platform === "darwin"
            ? ["open", [url]]
            : ["xdg-open", [url]];
      try {
        spawn(opener[0], opener[1], {
          detached: true,
          stdio: "ignore",
        }).unref();
      } catch {}
    });
    setTimeout(() => {
      srv.close();
      reject(new Error("login timed out (2m)"));
    }, 120_000);
  });

  const v = await tokenRequest({
    grant_type: "authorization_code",
    code,
    client_id: CLIENT_ID,
    redirect_uri: redirect,
    code_verifier: verifier,
  });
  const claims = accountFromJwt(v.id_token ?? v.access_token ?? "");
  const auth: AuthFile = {
    access_token: v.access_token,
    refresh_token: v.refresh_token,
    expires_at: v.expires_in ? Date.now() + v.expires_in * 1000 : undefined,
    account_id: claims.account,
    email: claims.email,
  };
  saveAuth(home, auth);
  return auth;
}
