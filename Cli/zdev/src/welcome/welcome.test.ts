// The first-run flow, driven headlessly: title art, every screen at full
// size, keys → live models → default, rejected keys, offline fallback.
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import type { ModelDef, ProviderDef } from "../catalog";
import * as creds from "../creds";
import * as live from "../live";
import { loadConfig } from "../store";
import type { Key } from "../ui/keys";
import { logoGrid } from "../logo";
import { Canvas } from "./screen";
import { synth, SOUND_SECS, wav } from "./sound";
import { Flow, titleArt } from "./index";

let home = "";
const saved: Record<string, string | undefined> = {};
const ENV = ["COSMOS_CLI_HOME", "COSMOS_CLI_KEYRING", "ANTHROPIC_API_KEY", "OPENROUTER_API_KEY", "OPENAI_API_KEY", "ROUTER9_API_KEY"];
beforeEach(() => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "zdev-welcome-"));
  for (const k of ENV) saved[k] = process.env[k];
  for (const k of ENV) delete process.env[k];
  process.env.COSMOS_CLI_HOME = home;
  process.env.COSMOS_CLI_KEYRING = "0";
});
afterEach(() => {
  for (const k of ENV) (saved[k] === undefined ? delete process.env[k] : (process.env[k] = saved[k]));
  fs.rmSync(home, { recursive: true, force: true });
});

const key = (name: string, text?: string): Key => ({ name, text, ctrl: false, meta: false, shift: false });
const type = (f: Flow, s: string) => f.onKey(key("text", s));
const screen = (f: Flow) => f.frame(0.016)[0].plain();

const models = (def: ProviderDef): Promise<ModelDef[]> =>
  Promise.resolve(
    [`${def.id}-live-best`, "second", "third"].map((id) => ({ id, name: id, context: 200_000, output: 8192, reasoning: false, price: [3, 15, 0, 0] as [number, number, number, number] })),
  );
const rejected = (def: ProviderDef): Promise<ModelDef[]> => Promise.reject(new live.RejectedKey(`${def.base_url.split("/")[2]} rejected the key (401)`));
const offline = (): Promise<ModelDef[]> => Promise.reject(new Error("couldn't reach the host"));

async function settle(f: Flow) {
  for (let i = 0; i < 50 && f.pick?.loading; i++) await new Promise((r) => setTimeout(r, 5));
}

function toKeys(f: Flow) {
  f.onKey(key("text", "x")); // skip warp
  f.onKey(key("text", "x")); // skip coalesce
  expect(f.phase).toBe("name");
  f.onKey(key("return"));
  expect(f.phase).toBe("keys");
}

function select(f: Flow, id: string) {
  type(f, id);
  const i = f.provOrder()[0];
  expect(f.provs[i].def.id).toBe(id);
  f.onKey(key("return"));
  return i;
}

describe("art", () => {
  test("the title fits every size and lands on the braille grid", () => {
    for (const [w, h] of [[200, 40], [160, 16], [120, 16], [80, 14], [60, 8], [40, 6]]) {
      const a = titleArt(w, h);
      expect(a.cells.length).toBeGreaterThan(0);
      expect(a.rows).toBeLessThanOrEqual(Math.max(h, 2));
      if (w >= 52) expect(a.cols + 2).toBeLessThanOrEqual(w);
      expect(a.cells.every(([c, r, ch]) => c < a.cols && r < a.rows && ch !== "⠀")).toBe(true);
    }
    expect(titleArt(200, 40).rows).toBe(6);
    expect(titleArt(120, 16).rows).toBe(4);
    expect(titleArt(80, 14).rows).toBe(9);
    expect(titleArt(60, 8).rows).toBe(2);
  });

  test("the header mark is zcli's galaxy, cell for cell", () => {
    // rust-tui/crates/ztui/src/logo.rs at 12×4
    expect(logoGrid().map((r) => r.map((c) => c.ch).join(""))).toEqual([
      "    ⢀⣄⣤⣤⣥⣥⣄⡀",
      "⣴⡃ ⣴⣿⣯⣽⣬⣍⠙⠻⣷",
      "⢿⣷⣤⣙⣻⣛⣻⣿⡿⠁⢀⠚",
      "⠈⠙⠛⠿⠛⠛⠋⠉    ",
    ]);
  });

  test("the sound is zcli's: bounded, audible, full length, valid wav", () => {
    const s = synth(8000);
    expect(s.length).toBe(Math.floor(8000 * SOUND_SECS));
    expect(s.every((x) => Math.abs(x) <= 1)).toBe(true);
    expect(s.some((x) => Math.abs(x) > 0.05)).toBe(true);
    const w = wav(s, 8000);
    expect(w.toString("ascii", 0, 4)).toBe("RIFF");
    expect(w.length).toBe(44 + s.length * 2);
  });

  test("canvas clips and handles wide characters", () => {
    const c = new Canvas(6, 2);
    expect(c.text(0, 0, "你好世界")).toBe(6);
    expect(c.get(1, 0)).toBe("");
    c.text(4, 1, "abcdef");
    expect(c.plain()).toBe("你好世\n    ab");
  });
});

describe("flow", () => {
  test("every phase uses the whole screen", () => {
    const f = new Flow(120, 40);
    f.fetcher = models;
    for (let i = 0; i < 30; i++) f.frame(0.05);
    const warp = f.frame(0.05)[0];
    let lit = 0;
    for (let y = 0; y < 40; y++) for (let x = 0; x < 120; x++) if (warp.get(x, y) !== " ") (lit++, (x = 120));
    expect(lit).toBeGreaterThan(5);
    f.beginCoalesce();
    f.beginName();
    const name = screen(f);
    expect(name).toContain("what should i call you?");
    expect(name).toContain("zdev v0.1.0");
    expect([...name].some((ch) => ch >= "⠁" && ch <= "⣿")).toBe(true);
    f.onKey(key("return"));
    const keys = screen(f);
    for (const s of ["Bring your own keys", "ChatGPT", "OpenRouter", "9Router", "✓ name", "start", "Mistral"]) expect(keys).toContain(s);
  });

  test("a resize never skips the intro (Windows sends one when the screen opens)", () => {
    const f = new Flow(120, 30);
    f.frame(0.016);
    f.resized(120, 30);
    expect(f.phase).toBe("warp");
    f.resized(100, 40);
    expect(f.phase).toBe("warp");
    f.beginCoalesce();
    f.resized(140, 36);
    expect(f.phase).toBe("coalesce");
  });

  test("name → keys → live models → default model, shared with zcli", async () => {
    const f = new Flow(100, 30);
    f.fetcher = models;
    f.onKey(key("text", "x"));
    f.onKey(key("text", "x"));
    type(f, "Rav");
    f.onKey(key("return"));
    const i = select(f, "anthropic");
    expect(f.phase).toBe("key");
    f.onKey(key("paste", "sk-ant-test-123"));
    const s = screen(f);
    expect(s).not.toContain("sk-ant");
    expect(s).toContain("https://console.anthropic.com/settings/keys");
    f.onKey(key("return"));
    expect(f.phase).toBe("models");
    await settle(f);
    const m = screen(f);
    expect(m).toContain("anthropic-live-best");
    expect(m).toContain("3 models live from api.anthropic.com");
    expect(m).toContain("$3 / $15");
    type(f, "second");
    f.onKey(key("return"));
    expect(f.phase).toBe("keys");
    expect(f.model).toEqual(["anthropic/second", true]);
    expect(screen(f)).toContain("✓ second ★");
    expect(creds.storedKey("anthropic")?.[0]).toBe("sk-ant-test-123");
    expect(loadConfig(home).default_model).toBe("anthropic/second");
    expect(JSON.parse(fs.readFileSync(path.join(home, "zcli_state.json"), "utf8")).model).toBe("anthropic/second");
    expect(live.cached("anthropic")).toHaveLength(3);
    f.onKey(key("escape"));
    expect(f.phase).toBe("ready");
    expect(screen(f)).toContain("You're all set, Rav.");
    f.onKey(key("return"));
    expect(f.done && !f.quit).toBe(true);
    expect(loadConfig(home).user_name).toBe("Rav");
    expect(i).toBeGreaterThanOrEqual(0);
  });

  test("9Router is offered and connects like any key", async () => {
    const f = new Flow(100, 30);
    f.fetcher = models;
    toKeys(f);
    select(f, "router9");
    expect(screen(f)).toContain("localhost:20128");
    f.onKey(key("paste", "r9-key"));
    f.onKey(key("return"));
    await settle(f);
    expect(screen(f)).toContain("router9-live-best");
  });

  test("a rejected key is dropped and asked for again", async () => {
    const f = new Flow(100, 30);
    f.fetcher = rejected;
    toKeys(f);
    const i = select(f, "openrouter");
    f.onKey(key("paste", "sk-or-bad"));
    f.onKey(key("return"));
    for (let n = 0; n < 50 && f.phase === "models"; n++) await new Promise((r) => setTimeout(r, 5));
    expect(f.phase).toBe("key");
    expect(f.keyIdx).toBe(i);
    expect(creds.storedKey("openrouter")).toBeNull();
    expect(screen(f)).toContain("rejected the key");
  });

  test("offline falls back to the known models and keeps the key", async () => {
    const f = new Flow(100, 30);
    f.fetcher = offline;
    toKeys(f);
    const i = select(f, "openai");
    f.onKey(key("paste", "sk-test"));
    f.onKey(key("return"));
    await settle(f);
    expect(screen(f)).toContain("showing known models");
    expect(f.keyed.has("openai")).toBe(true);
    f.onKey(key("return"));
    expect(f.model).toEqual([`openai/${f.provs[i].def.models[0].id}`, true]);
  });

  test("continue without choosing prefers the provider connected in this run", async () => {
    const f = new Flow(100, 30);
    f.fetcher = models;
    toKeys(f);
    select(f, "anthropic");
    f.onKey(key("paste", "sk-ant-x"));
    f.onKey(key("return"));
    await settle(f);
    f.onKey(key("escape"));
    expect(f.phase).toBe("keys");
    f.onKey(key("escape"));
    expect(f.model).toEqual(["anthropic/anthropic-live-best", true]);
  });

  test("ctrl+c quits without marking anything", () => {
    const f = new Flow(100, 30);
    f.onKey({ name: "c", ctrl: true, meta: false, shift: false });
    expect(f.done && f.quit).toBe(true);
  });
});
