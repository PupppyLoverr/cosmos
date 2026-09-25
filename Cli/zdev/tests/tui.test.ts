// The real TUI in a real terminal emulator: <App/> renders into a headless
// xterm (so Ink's <Static> scrollback, erasing and redraws are all
// interpreted), keys go in as raw bytes, and the screen is read back.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import React from "react";
import { render } from "ink";
import { Terminal } from "@xterm/headless";

import { RunOpts } from "../src/proto";
import { App } from "../src/ui/app";
import type { ZdevDriver } from "../src/ui/driver";

const COLS = 100;
const ROWS = 30;

class Out extends EventEmitter {
  isTTY = true;
  constructor(
    private term: Terminal,
    public columns: number,
    public rows: number,
  ) {
    super();
  }
  write(d: string | Uint8Array, cb?: unknown) {
    this.term.write(typeof d === "string" ? d : Buffer.from(d).toString("utf8"));
    if (typeof cb === "function") (cb as () => void)();
    return true;
  }
}

class In extends EventEmitter {
  isTTY = true;
  private q: string[] = [];
  setRawMode() {
    return this;
  }
  setEncoding() {
    return this;
  }
  ref() {
    return this;
  }
  unref() {
    return this;
  }
  resume() {
    return this;
  }
  pause() {
    return this;
  }
  read() {
    return this.q.shift() ?? null;
  }
  type(s: string) {
    this.q.push(s);
    this.emit("readable");
  }
}

let home = "";
let ws = "";
let term: Terminal;
let stdin: In;
let driver: ZdevDriver | null = null;
let app: ReturnType<typeof render>;
const env: Record<string, string | undefined> = {};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const flush = () => new Promise<void>((r) => term.write("", () => r()));

/** Whole buffer (scrollback + viewport), trailing blanks trimmed. */
async function all(): Promise<string> {
  await flush();
  const b = term.buffer.active;
  const out: string[] = [];
  for (let i = 0; i < b.length; i++) out.push(b.getLine(i)?.translateToString(true) ?? "");
  while (out.length && !out[out.length - 1].trim()) out.pop();
  return out.join("\n");
}

async function screen(): Promise<string> {
  await flush();
  const b = term.buffer.active;
  const out: string[] = [];
  for (let i = 0; i < ROWS; i++) out.push(b.getLine(b.viewportY + i)?.translateToString(true) ?? "");
  return out.join("\n");
}

async function until(pred: (s: string) => boolean, ms = 4000): Promise<string> {
  const end = Date.now() + ms;
  let s = "";
  while (Date.now() < end) {
    s = await all();
    if (pred(s)) return s;
    await sleep(25);
  }
  throw new Error(`timed out; screen was:\n${await screen()}`);
}

async function type(s: string) {
  stdin.type(s);
  await sleep(60);
}

beforeAll(async () => {
  // a clean machine: no provider keys from the real environment
  const keys = ["ROUTER9_API_KEY", "OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"];
  for (const k of ["COSMOS_CLI_HOME", "COSMOS_CLI_KEYRING", "MERIDIAN_BIN", ...keys]) env[k] = process.env[k];
  for (const k of keys) delete process.env[k];
  home = fs.mkdtempSync(path.join(os.tmpdir(), "zdev-tui-home-"));
  ws = fs.mkdtempSync(path.join(os.tmpdir(), "zdev-tui-ws-"));
  fs.writeFileSync(path.join(ws, "a.txt"), "hello from a\n");
  process.env.COSMOS_CLI_HOME = home;
  process.env.COSMOS_CLI_KEYRING = "0";
  process.env.MERIDIAN_BIN = path.join(ws, "no-meridian.exe");
  term = new Terminal({ cols: COLS, rows: ROWS, scrollback: 2000, allowProposedApi: true, convertEol: true });
  const out = new Out(term, COLS, ROWS);
  stdin = new In();
  const opts = new RunOpts();
  opts.cwd = ws;
  opts.no_sandbox = true;
  app = render(React.createElement(App, { boot: { cwd: ws, opts }, onExit: () => {}, onDriver: (d: ZdevDriver) => (driver = d) }), {
    stdout: out as any,
    stderr: out as any,
    stdin: stdin as any,
    exitOnCtrlC: false,
    patchConsole: false,
  });
  await until((s) => s.includes("Welcome back") && s.includes("No model connected yet"));
});

afterAll(async () => {
  // ZDEV_TUI_DUMP=1 prints the final terminal (scrollback included) to eyeball it
  if (process.env.ZDEV_TUI_DUMP) console.log(await all());
  app?.unmount();
  for (const [k, v] of Object.entries(env)) (v === undefined ? delete process.env[k] : (process.env[k] = v));
  fs.rmSync(home, { recursive: true, force: true });
  fs.rmSync(ws, { recursive: true, force: true });
});

describe("zdev TUI in a terminal emulator", () => {
  test("boots with the galaxy header once, a note and the composer", async () => {
    const s = await all();
    expect(s).toContain("zdev 0.1.0");
    expect(s).toContain("⣾⣿⣿⣿⠻⣟⣍⣋"); // the zcli galaxy, sampled at 24×8
    expect(s.match(/Welcome back/g)).toHaveLength(1);
    expect(s).toContain("⏵⏵ agent");
    expect(s).toContain("Ask zdev to build, fix or explain");
  });

  test("a prompt runs: user chip, reply, turn footer", async () => {
    await type("ping");
    await type("\r");
    const s = await until((x) => x.includes("pong") && x.includes("Done in"));
    expect(s).toContain("❯ ping");
    // the header was committed once and stays once
    expect(s.match(/Welcome back/g)).toHaveLength(1);
  });

  test("allowed tools run and summarize; results render once", async () => {
    await type('tool:read {"path":"a.txt"}');
    await type("\r");
    const s = await until((x) => x.includes("Read(a.txt)") && (x.match(/Done in/g) ?? []).length >= 2);
    expect(s).toContain("Read 1 line");
    expect(s.match(/Read\(a\.txt\)/g)).toHaveLength(1);
  });

  test("an ask-tool shows the permission prompt; 1 allows it", async () => {
    await type('tool:bash {"command":"echo zdev-harness-ok"}');
    await type("\r");
    await until((x) => x.includes("Do you want to proceed?"));
    await type("1");
    const s = await until((x) => x.includes("zdev-harness-ok") && (x.match(/Done in/g) ?? []).length >= 3, 8000);
    expect(s).toContain("Bash(echo zdev-harness-ok)");
  });

  test("a streamed reply renders as it arrives and commits whole paragraphs", async () => {
    // a local OpenAI-compatible endpoint that streams a long reply slowly
    const parts = ["First paragraph ", "streams in.\n\n", "Second para", "graph grows.\n\n```ts\nconst a = 1;\n\nconst b = 2;\n", "```\n\nTail."];
    const srv = Bun.serve({
      port: 0,
      fetch() {
        const enc = new TextEncoder();
        const body = new ReadableStream({
          async start(c) {
            for (const p of parts) {
              c.enqueue(enc.encode(`data: ${JSON.stringify({ choices: [{ delta: { content: p } }] })}\n\n`));
              await sleep(150);
            }
            c.enqueue(enc.encode(`data: ${JSON.stringify({ choices: [], usage: { prompt_tokens: 10, completion_tokens: 30 } })}\n\ndata: [DONE]\n\n`));
            c.close();
          },
        });
        return new Response(body, { headers: { "content-type": "text/event-stream" } });
      },
    });
    try {
      const a = driver!.agent!;
      a.provider = { kind: "openai", base_url: `http://127.0.0.1:${srv.port}/v1`, api_key_env: "", model: "mock", context: 100000, prices: {} };
      a.allowed = ["127.0.0.1"];
      await type("tell me a story");
      await type("\r");
      // mid-stream: the first paragraph is already on screen, the end isn't
      const mid = await until((x) => x.includes("First paragraph streams in."));
      expect(mid).not.toContain("Tail.");
      const s = await until((x) => x.includes("Tail.") && (x.match(/Done in/g) ?? []).length >= 4, 8000);
      expect(s.match(/First paragraph streams in\./g)).toHaveLength(1);
      expect(s).toContain("Second paragraph grows.");
      // the fence stayed whole: both code lines under one "ts" label, Tail outside it
      expect(s).toMatch(/ts\n\s*│ const a = 1;\n\s*│\s*\n\s*│ const b = 2;/);
      expect(s).toMatch(/\n\s+Tail\./);
      expect(s).not.toMatch(/│ Tail/);
    } finally {
      srv.stop(true);
    }
  });

  test("slash menu filters; tab completes; esc clears", async () => {
    await type("/mo");
    let s = await screen();
    expect(s).toContain("/model");
    expect(s).toContain("/mode");
    await type("\t");
    s = await screen();
    expect(s).toMatch(/❯ \/model?$/m);
    await type("\x1b");
    await sleep(50);
    s = await screen();
    expect(s).toContain("Ask zdev to build");
  });

  test("held arrow keys arrive as one chunk and still work (history)", async () => {
    await type("\x1b[A\x1b[A");
    const s = await screen();
    // two steps back in history (…, bash prompt, story prompt), not escape garbage
    expect(s).toContain('❯ tool:bash {"command":"echo zdev-harness-ok"}');
    expect(s).not.toContain("[A");
    await type("\x15"); // ctrl+u clears the line
  });

  test("a bracketed paste with newlines is inserted, not sent", async () => {
    await type("\x1b[200~line one\rline two\x1b[201~");
    const s = await screen();
    expect(s).toContain("❯ line one");
    expect(s).toContain("  line two");
    expect((await all()).match(/❯ line one/g)).toHaveLength(1); // not submitted
    await type("\x03"); // ctrl+c clears the input
    expect(await screen()).toContain("Ask zdev to build");
  });

  test("/connect lists 9Router and OpenRouter and asks for a key", async () => {
    await type("/connect");
    await type("\r");
    let s = await until((x) => x.includes("Connect a provider"));
    for (const p of ["ChatGPT", "OpenRouter", "9Router", "Anthropic"]) expect(s).toContain(p);
    await type("9router");
    await type("\r");
    s = await until((x) => x.includes("9Router API key"));
    expect(s).toContain("localhost:20128");
    await type("\x1b");
    await type("\x1b");
  });

  test("? opens the key sheet: it fits, scrolls, and closes without leaving a trace", async () => {
    await type("?");
    let s = await until((x) => x.includes("keys and commands"));
    expect(s).toContain("shift+tab");
    for (let i = 0; i < 30; i++) await type("\x1b[B");
    s = await screen();
    expect(s).toContain("/quit"); // scrolled to the end
    await type("x");
    await sleep(80);
    expect(await all()).not.toContain("keys and commands"); // nothing stuck in scrollback either
    expect(await screen()).toContain("Ask zdev to build");
  });

  test("a long code block streaming in a short terminal never overflows it", async () => {
    const code = Array.from({ length: 70 }, (_, i) => `line_${String(i).padStart(2, "0")} = ${i}`).join("\n");
    const text = `Here it comes:\n\n\`\`\`py\n${code}\n\`\`\`\n\nAll ${70} lines.`;
    const parts: string[] = [];
    for (let i = 0; i < text.length; i += 90) parts.push(text.slice(i, i + 90));
    const srv = Bun.serve({
      port: 0,
      fetch() {
        const enc = new TextEncoder();
        return new Response(
          new ReadableStream({
            async start(c) {
              for (const p of parts) {
                c.enqueue(enc.encode(`data: ${JSON.stringify({ choices: [{ delta: { content: p } }] })}\n\n`));
                await sleep(15);
              }
              c.enqueue(enc.encode("data: [DONE]\n\n"));
              c.close();
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        );
      },
    });
    try {
      const a = driver!.agent!;
      a.provider = { kind: "openai", base_url: `http://127.0.0.1:${srv.port}/v1`, api_key_env: "", model: "mock", context: 100000, prices: {} };
      a.allowed = ["127.0.0.1"];
      await type("dump it");
      await type("\r");
      const mid = await until((x) => x.includes("more lines above"), 8000);
      expect(mid).toContain("line_");
      const s = await until((x) => x.includes("All 70 lines.") && (x.match(/Done in/g) ?? []).length >= 5, 10000);
      // every line exactly once in the terminal, in order; no streaming leftovers
      for (let i = 0; i < 70; i++) expect(s.match(new RegExp(`line_${String(i).padStart(2, "0")} =`, "g"))).toHaveLength(1);
      expect(s).not.toContain("more lines above");
    } finally {
      srv.stop(true);
    }
  });

  test("shift+tab cycles the mode", async () => {
    await type("\x1b[Z");
    await until((x) => x.includes("plan mode"));
    await type("\x1b[Z");
    await until((x) => x.includes("⏵⏵ yolo"));
    await type("\x1b[Z");
    await until((x) => x.includes("⏵⏵ agent"));
  });
});
