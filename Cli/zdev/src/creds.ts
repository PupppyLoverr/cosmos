// Credentials shared with zcli (rust-tui/crates/zcore/src/creds.rs).
//
// An API key resolves from the provider's env vars, then the OS credential
// store (Windows Credential Manager, target "<id>.cosmos-cli"), then
// COSMOS_CLI_HOME/credentials.json. New keys go to the OS store when it
// works, else to the file. COSMOS_CLI_KEYRING=0 forces the file.
//
// The ChatGPT sign-in lives in credentials.json under "chatgpt" in zcli's
// shape (expires_at in unix seconds), so signing in once covers both CLIs.
import fs from "node:fs";
import path from "node:path";

import { cliHome } from "./proto";
import * as wincred from "./wincred";

export type Source = "environment" | "credential store" | "credentials.json";

export type ChatgptTokens = {
  access: string;
  refresh: string;
  /** unix seconds */
  expires_at: number;
  account_id: string;
  email?: string | null;
  plan?: string | null;
};

type CredFile = { keys: Record<string, string>; chatgpt?: ChatgptTokens };

const file = () => path.join(cliHome(), "credentials.json");

export function loadFile(): CredFile {
  try {
    const v = JSON.parse(fs.readFileSync(file(), "utf8"));
    return { keys: v?.keys && typeof v.keys === "object" ? v.keys : {}, chatgpt: v?.chatgpt ?? undefined };
  } catch {
    return { keys: {} };
  }
}

export function saveFile(f: CredFile): void {
  const p = file();
  fs.mkdirSync(path.dirname(p), { recursive: true });
  const out: Record<string, unknown> = { keys: f.keys };
  if (f.chatgpt) out.chatgpt = f.chatgpt;
  const tmp = p + ".tmp";
  fs.writeFileSync(tmp, JSON.stringify(out, null, 2), "utf8");
  fs.renameSync(tmp, p);
}

function keyringEnabled(): boolean {
  return process.platform === "win32" && process.env.COSMOS_CLI_KEYRING !== "0" && wincred.available();
}

/** A stored (not environment) key for `id`. */
export function storedKey(id: string): [string, Source] | null {
  if (keyringEnabled()) {
    const k = wincred.read(id);
    if (k) return [k, "credential store"];
  }
  const k = loadFile().keys[id];
  return k ? [k, "credentials.json"] : null;
}

/** Resolve a key: env vars first, then the stores. */
export function keyFor(id: string, env: string[]): [string, Source] | null {
  for (const v of env) {
    const val = (process.env[v] ?? "").trim();
    if (val) return [val, "environment"];
  }
  return storedKey(id);
}

/** Save a key; returns where it went. */
export function storeKey(id: string, key: string): Source {
  key = key.trim();
  if (!key) throw new Error("empty key");
  if (keyringEnabled() && wincred.write(id, key)) {
    // don't leave a stale copy in the file
    const f = loadFile();
    if (id in f.keys) {
      delete f.keys[id];
      saveFile(f);
    }
    return "credential store";
  }
  const f = loadFile();
  f.keys[id] = key;
  saveFile(f);
  return "credentials.json";
}

/** Forget a stored key (environment variables are left alone). */
export function deleteKey(id: string): void {
  if (keyringEnabled()) wincred.remove(id);
  const f = loadFile();
  if (id in f.keys) {
    delete f.keys[id];
    saveFile(f);
  }
}

/** Show only the ends of a key. */
export function mask(key: string): string {
  const chars = [...key];
  if (chars.length <= 10) return "•".repeat(Math.max(4, chars.length));
  return `${chars.slice(0, 4).join("")}…${chars.slice(-4).join("")}`;
}

// ── ChatGPT sign-in ─────────────────────────────────────────────────────────

/** The signed-in ChatGPT session. Migrates zdev's old openai-auth.json once. */
export function chatgpt(): ChatgptTokens | null {
  const f = loadFile();
  if (f.chatgpt?.access) return f.chatgpt;
  const legacy = path.join(cliHome(), "openai-auth.json");
  try {
    const a = JSON.parse(fs.readFileSync(legacy, "utf8"));
    if (!a?.access_token) return null;
    const t: ChatgptTokens = {
      access: a.access_token,
      refresh: a.refresh_token ?? "",
      expires_at: a.expires_at ? Math.floor(a.expires_at / 1000) : 0,
      account_id: a.account_id ?? "",
      email: a.email ?? null,
      plan: null,
    };
    saveChatgpt(t);
    try {
      fs.unlinkSync(legacy);
    } catch {}
    return t;
  } catch {
    return null;
  }
}

export function saveChatgpt(t: ChatgptTokens): void {
  const f = loadFile();
  f.chatgpt = t;
  saveFile(f);
}

export function clearChatgpt(): void {
  const f = loadFile();
  if (!f.chatgpt) return;
  delete f.chatgpt;
  saveFile(f);
}
