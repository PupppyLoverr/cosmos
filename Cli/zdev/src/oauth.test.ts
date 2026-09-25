import { describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { loadAuth, saveAuth, clearAuth, accessToken } from "./oauth";
import { allowedHosts, loadConfig } from "./store";
import { BYOK } from "./byok";

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), "zdev-oauth-"));

describe("oauth token store", () => {
  test("save → load roundtrip; clear removes", () => {
    const home = tmp();
    saveAuth(home, {
      access_token: "a",
      refresh_token: "r",
      expires_at: Date.now() + 3600_000,
      account_id: "acc",
      email: "x@y.z",
    });
    const a = loadAuth(home);
    expect(a?.access_token).toBe("a");
    expect(a?.email).toBe("x@y.z");
    clearAuth(home);
    expect(loadAuth(home)).toBeNull();
  });

  test("accessToken errors when signed out; returns fresh token unrefreshed", async () => {
    const home = tmp();
    await expect(accessToken(home)).rejects.toThrow("not signed in");
    saveAuth(home, {
      access_token: "fresh",
      expires_at: Date.now() + 3600_000,
    });
    const a = await accessToken(home);
    expect(a.access_token).toBe("fresh");
  });
});

describe("byok catalog", () => {
  test("every provider has sane defaults; oauth/keyless entries need no env", () => {
    const seen = new Set<string>();
    for (const p of BYOK) {
      expect(seen.has(p.id)).toBe(false);
      seen.add(p.id);
      expect(p.alias).toBeTruthy();
      expect(p.model).toBeTruthy();
      expect(p.context).toBeGreaterThan(0);
      if (!p.oauth && p.id !== "fixture" && p.base_url)
        expect(p.base_url).toMatch(/^https?:\/\//);
      if (!p.oauth && !p.env) expect(["ollama", "fixture"]).toContain(p.id);
    }
    // headliners present
    for (const id of ["chatgpt", "openai", "anthropic", "google", "meta", "mimo"])
      expect(seen.has(id)).toBe(true);
  });
});

describe("chatgpt kind", () => {
  test("allowlist includes the OAuth issuer", () => {
    const home = tmp();
    fs.writeFileSync(
      path.join(home, "config.toml"),
      '[providers.chatgpt]\nkind = "chatgpt"\nbase_url = "https://chatgpt.com/backend-api/codex"\n',
    );
    const cfg = loadConfig(home);
    const hosts = allowedHosts(cfg);
    expect(hosts).toContain("chatgpt.com");
    expect(hosts).toContain("auth.openai.com");
  });
});
