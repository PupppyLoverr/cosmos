// zdev conformance tests — port of python-cloud/tests/test_zdev.py:
// tools, sessions, event schema + plan-mode deny and fixture termination.
import { afterEach, describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import * as proto from "../src/proto";
import * as store from "../src/store";
import * as t from "../src/tools";
import { Agent } from "../src/agent";
import { hostAllowed } from "../src/providers";

const tmps: string[] = [];
function tmpdir(): string {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), "zdev-test-"));
  tmps.push(d);
  return d;
}
afterEach(() => {
  while (tmps.length) {
    const d = tmps.pop()!;
    try {
      fs.rmSync(d, { recursive: true, force: true });
    } catch {}
  }
});

test("read hashlines", () => {
  const tmp = tmpdir();
  fs.writeFileSync(path.join(tmp, "a.txt"), "alpha\nbeta\ngamma\n");
  const ctx = new t.Ctx(tmp);
  const out = t.run(ctx, "read", { path: "a.txt" });
  expect(out).toContain("file_sha256=");
  expect(out).toContain("lines=3");
  expect(out).toContain("|alpha");
  expect(out).toContain("1:");
});

test("write/read/edit round-trip", () => {
  const tmp = tmpdir();
  const ctx = new t.Ctx(tmp);
  t.run(ctx, "write", { path: "b.txt", content: "one\ntwo\nthree\n" });
  const r = t.run(ctx, "read", { path: "b.txt" });
  const anchor = r
    .split("\n")
    .find((l) => l.includes("|two"))!
    .split("|")[0];
  const e = t.run(ctx, "edit", {
    path: "b.txt",
    edits: [{ start: anchor, end: anchor, new_text: "TWO" }],
  });
  expect(e.startsWith("ok")).toBe(true);
  expect(fs.readFileSync(path.join(tmp, "b.txt"), "utf8")).toBe(
    "one\nTWO\nthree\n",
  );
});

test("stale guard + escape deny", () => {
  const tmp = tmpdir();
  fs.writeFileSync(path.join(tmp, "c.txt"), "x\ny\n");
  const ctx = new t.Ctx(tmp);
  const e = t.run(ctx, "edit", {
    path: "c.txt",
    file_sha256: "deadbeef",
    edits: [{ start: "1:aaaaaa", end: "1:aaaaaa", new_text: "z" }],
  });
  expect(e).toContain("STALE");
  const e2 = t.run(ctx, "write", { path: "c.txt", content: "clobber" });
  expect(e2).toContain("STALE");
  const esc = t.run(ctx, "read", { path: "../outside.txt" });
  expect(esc).toContain("escapes workspace");
});

test("session append/resume + branch", () => {
  const home = tmpdir();
  const s = store.Session.create(home, "zdev");
  s.append("session_start", { model: "fixture" });
  s.append("user", { text: "ping" });
  s.append("assistant", { text: "pong", tool_calls: [] });
  const reopened = store.Session.open(home, s.id);
  expect(reopened.harness).toBe("zdev");
  const msgs = reopened.replay();
  expect(msgs).toHaveLength(2);
  expect(msgs[0].role).toBe("user");
  const s2 = store.Session.open(home, s.id);
  const evs = s2.events();
  s2.branch(evs[1].id);
  const msgs2 = store.Session.open(home, s.id).replay();
  expect(msgs2).toHaveLength(1);
});

test("event shape", () => {
  const s = store.Session.create(tmpdir(), "zdev");
  const ev = s.append("user", { text: "hi" });
  for (const k of ["v", "id", "parent", "ts", "session", "harness", "kind", "data"]) {
    expect(ev).toHaveProperty(k);
  }
  expect(ev.v).toBe(1);
  expect(ev.harness).toBe("zdev");
  expect(ev.kind).toBe("user");
  expect(ev.id).toMatch(/^[0-9a-z]{26}$/);
});

test("tools dump byte-identical to contracts/tools.json", () => {
  const contract = path.resolve(__dirname, "..", "..", "contracts", "tools.json");
  const canonical = fs.readFileSync(contract, "utf8");
  expect(t.toolsDump()).toBe(canonical);
});

test("host allowlist", () => {
  hostAllowed("https://api.openai.com/x", ["api.openai.com"]);
  for (const bad of ["https://evil.com", "https://api.openai.com.evil.com"]) {
    expect(() => hostAllowed(bad, ["api.openai.com"])).toThrow(/allowlist/);
  }
});

test("plan-mode deny", async () => {
  const home = tmpdir();
  process.env.COSMOS_CLI_HOME = home;
  const ws = tmpdir();
  fs.writeFileSync(path.join(ws, "v.txt"), "keep");
  const o = new proto.RunOpts();
  o.cwd = ws;
  o.mode = "plan";
  o.model = "fixture";
  const ag = new Agent(o, "zdev", ws);
  try {
    const [text, isError] = await ag.execTool(
      "t1",
      "write",
      { path: "v.txt", content: "x" },
      () => {},
    );
    expect(isError).toBe(true);
    expect(text).toMatch(/denied|plan/);
  } finally {
    ag.cleanup();
  }
});

test("fixture tool call terminates", async () => {
  const home = tmpdir();
  process.env.COSMOS_CLI_HOME = home;
  const ws = tmpdir();
  fs.writeFileSync(path.join(ws, "a.txt"), "hello");
  const o = new proto.RunOpts();
  o.cwd = ws;
  o.model = "fixture";
  o.allow = ["read"];
  const ag = new Agent(o, "zdev", ws);
  try {
    ag.sendUser('tool:read {"path":"a.txt"}', () => {});
    const done1 = await ag.turn(() => {});
    expect(done1).toBe(false);
    const calls = ag.messages[ag.messages.length - 1].content.filter(
      (b) => b.type === "tool_use",
    );
    expect(calls).toHaveLength(1);
    await ag.execTool(calls[0].id, calls[0].name, calls[0].input, () => {});
    const done2 = await ag.turn(() => {});
    expect(done2).toBe(true); // "done." — no further tool calls
  } finally {
    ag.cleanup();
  }
});
