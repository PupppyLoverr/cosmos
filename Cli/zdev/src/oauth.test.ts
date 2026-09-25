// Shared credentials, the ChatGPT sign-in, the catalog and model resolution.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import * as catalog from "./catalog";
import * as creds from "./creds";
import * as live from "./live";
import { authorizeUrl, pkce, tokensFrom } from "./oauth";
import { allowedHosts, availableModels, loadConfig, projectInstructions, providerFor, setDefaultModel, setUserName } from "./store";

let home = "";
const saved: Record<string, string | undefined> = {};
const ENV = ["COSMOS_CLI_HOME", "COSMOS_CLI_KEYRING", "ANTHROPIC_API_KEY", "OPENROUTER_API_KEY", "ROUTER9_API_KEY", "OPENAI_API_KEY"];

beforeEach(() => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "zdev-creds-"));
  for (const k of ENV) saved[k] = process.env[k];
  for (const k of ENV) delete process.env[k];
  process.env.COSMOS_CLI_HOME = home;
  process.env.COSMOS_CLI_KEYRING = "0"; // file store — never touch the real credential manager
});
afterEach(() => {
  for (const k of ENV) {
    if (saved[k] === undefined) delete process.env[k];
    else process.env[k] = saved[k];
  }
  fs.rmSync(home, { recursive: true, force: true });
});

const jwt = (payload: object) => `x.${Buffer.from(JSON.stringify(payload)).toString("base64url")}.y`;

describe("credentials (shared with zcli)", () => {
  test("file store round trip, env wins, delete, mask", () => {
    expect(creds.keyFor("anthropic", ["ANTHROPIC_API_KEY"])).toBeNull();
    expect(creds.storeKey("anthropic", " sk-ant-123456789 ")).toBe("credentials.json");
    expect(creds.keyFor("anthropic", ["ANTHROPIC_API_KEY"])).toEqual(["sk-ant-123456789", "credentials.json"]);
    process.env.ANTHROPIC_API_KEY = "from-env";
    expect(creds.keyFor("anthropic", ["ANTHROPIC_API_KEY"])).toEqual(["from-env", "environment"]);
    creds.deleteKey("anthropic");
    expect(creds.storedKey("anthropic")).toBeNull();
    expect(creds.mask("sk-ant-123456789")).toBe("sk-a…6789");
    // zcli's file layout
    const f = JSON.parse(fs.readFileSync(path.join(home, "credentials.json"), "utf8"));
    expect(f).toHaveProperty("keys");
  });

  test("ChatGPT tokens use zcli's shape; the old openai-auth.json migrates", () => {
    fs.writeFileSync(
      path.join(home, "openai-auth.json"),
      JSON.stringify({ access_token: "a1", refresh_token: "r1", expires_at: 1_900_000_000_000, account_id: "acc", email: "x@y.z" }),
    );
    const t = creds.chatgpt()!;
    expect(t).toMatchObject({ access: "a1", refresh: "r1", expires_at: 1_900_000_000, account_id: "acc", email: "x@y.z" });
    expect(fs.existsSync(path.join(home, "openai-auth.json"))).toBe(false);
    expect(JSON.parse(fs.readFileSync(path.join(home, "credentials.json"), "utf8")).chatgpt.access).toBe("a1");
    creds.clearChatgpt();
    expect(creds.chatgpt()).toBeNull();
  });
});

describe("ChatGPT sign-in", () => {
  test("PKCE challenge and Codex authorize parameters", () => {
    const p = pkce();
    expect(p.verifier.length).toBeGreaterThanOrEqual(43);
    const u = authorizeUrl(p.challenge, "st8");
    expect(u.startsWith("https://auth.openai.com/oauth/authorize?")).toBe(true);
    for (const s of ["client_id=app_EMoamEEZ73f0CkXaXp7hrann", "code_challenge_method=S256", "state=st8", "codex_cli_simplified_flow=true"]) expect(u).toContain(s);
    expect(u).toContain(encodeURIComponent("http://localhost:1455/auth/callback"));
  });

  test("tokens read the account and plan claims", () => {
    const access = jwt({ "https://api.openai.com/auth": { chatgpt_account_id: "acc_1", chatgpt_plan_type: "plus" } });
    const t = tokensFrom({ access_token: access, refresh_token: "r", id_token: jwt({ email: "a@b.c" }), expires_in: 60 });
    expect(t).toMatchObject({ account_id: "acc_1", plan: "plus", email: "a@b.c", refresh: "r" });
    expect(() => tokensFrom({ access_token: jwt({}), refresh_token: "r" })).toThrow(/no ChatGPT account/);
  });
});

describe("catalog + model resolution", () => {
  test("the shared catalog has the headliners, 9Router and OpenRouter", () => {
    for (const id of ["chatgpt", "router9", "openrouter", "openai", "anthropic", "google", "xai", "meta", "xiaomi", "deepseek", "ollama"])
      expect(catalog.provider(id)).toBeDefined();
    expect(catalog.provider("router9")!.base_url).toBe("http://localhost:20128/v1");
    expect(catalog.splitRef("openrouter/anthropic/claude-x")).toEqual(["openrouter", "anthropic/claude-x"]);
    expect(catalog.splitRef("r9")).toBeNull();
  });

  test("provider/model refs resolve with stored keys; config aliases still work", () => {
    fs.writeFileSync(
      path.join(home, "config.toml"),
      '[providers.local]\nkind = "openai"\nbase_url = "http://localhost:1/v1"\n\n[models.mine]\nprovider = "local"\nid = "m1"\ncontext = 1000\n',
    );
    creds.storeKey("anthropic", "sk-ant-x");
    const cfg = loadConfig(home);
    const [p, id] = providerFor(cfg, "anthropic/claude-opus-5-5")!;
    expect(id).toBe("claude-opus-5-5");
    expect(p).toMatchObject({ kind: "anthropic", api_key: "sk-ant-x", base_url: "https://api.anthropic.com" });
    expect(providerFor(cfg, "mine")![1]).toBe("m1");
    expect(providerFor(cfg, "nope/x")).toBeNull();
    const models = availableModels(cfg);
    expect(models[0]).toMatchObject({ alias: "mine", fromConfig: true });
    expect(models.some((m) => m.provider === "anthropic")).toBe(true);
    expect(models.some((m) => m.provider === "openai")).toBe(false); // no key
    const hosts = allowedHosts(cfg);
    for (const h of ["api.anthropic.com", "openrouter.ai", "chatgpt.com", "auth.openai.com", "localhost"]) expect(hosts).toContain(h);
  });

  test("a cached live list replaces the snapshot in /model", () => {
    creds.storeKey("openrouter", "sk-or");
    live.store("openrouter", [{ id: "z/new-model", name: "New", context: 5, output: 1, reasoning: false, price: [0, 0, 0, 0] }]);
    const or = availableModels(loadConfig(home)).filter((m) => m.provider === "openrouter");
    expect(or.map((m) => m.alias)).toEqual(["openrouter/z/new-model"]);
  });

  test("default_model and user_name are top-level keys, written once", () => {
    fs.writeFileSync(path.join(home, "config.toml"), '[providers.r9]\nkind = "openai"\n');
    setDefaultModel("anthropic/claude-opus-5-5", home);
    setDefaultModel("openai/gpt-5.5", home);
    setUserName("Rav", home);
    const text = fs.readFileSync(path.join(home, "config.toml"), "utf8");
    expect(text.match(/default_model/g)).toHaveLength(1);
    const cfg = loadConfig(home);
    expect(cfg.default_model).toBe("openai/gpt-5.5");
    expect(cfg.user_name).toBe("Rav");
    expect(cfg.providers.r9).toBeDefined();
  });

  test("project instructions are read from AGENTS.md / CLAUDE.md", () => {
    const ws = fs.mkdtempSync(path.join(os.tmpdir(), "zdev-ws-"));
    expect(projectInstructions(ws)).toBeNull();
    fs.writeFileSync(path.join(ws, "AGENTS.md"), "use bun");
    fs.writeFileSync(path.join(ws, "CLAUDE.md"), "no emojis");
    const s = projectInstructions(ws)!;
    expect(s).toContain('<project-instructions file="AGENTS.md">\nuse bun');
    expect(s).toContain("no emojis");
    fs.rmSync(ws, { recursive: true, force: true });
  });
});

describe("live model lists", () => {
  test("merge drops non-chat models, dedupes and orders catalog-first", () => {
    const def = catalog.provider("openai")!;
    const known = def.models[1].id;
    const m = live.merge(def, [
      { id: "text-embedding-3-large", created: 9, reasoning: false },
      { id: "gpt-old", created: 1, reasoning: false },
      { id: "gpt-brand-new", created: 5, reasoning: false },
      { id: known, created: 0, reasoning: false },
      { id: "whisper-1", created: 0, reasoning: false },
      { id: "x:batch", created: 0, reasoning: false },
      { id: "gpt-old", created: 1, reasoning: false },
    ]);
    expect(m.map((x) => x.id)).toEqual([known, "gpt-brand-new", "gpt-old"]);
    expect(m[0].context).toBe(def.models[1].context);
    expect(m[1].context).toBe(128_000);
    expect(live.perM("0.000003")).toBe(3);
  });
});
