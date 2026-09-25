import { describe, expect, test } from "bun:test";
import { applyUpdate, settle, splitStreaming, summarizeArg } from "./reducer";
import { emptyState } from "./types";

describe("applyUpdate", () => {
  test("assistant chunks coalesce; empty chunks skipped", () => {
    let s = emptyState();
    s = applyUpdate(s, {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "hello" },
    });
    s = applyUpdate(s, {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: " world" },
    });
    s = applyUpdate(s, {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "" },
    });
    expect(s.items).toHaveLength(1);
    expect(s.items[0]).toMatchObject({ kind: "assistant", text: "hello world" });
  });

  test("tool_call → tool_call_update correlates by toolCallId", () => {
    let s = emptyState();
    s = applyUpdate(s, {
      sessionUpdate: "tool_call",
      toolCallId: "t1",
      title: "bash",
      input: { command: "echo hi" },
    });
    expect(s.items[0]).toMatchObject({
      kind: "tool",
      name: "bash",
      arg: "echo hi",
      status: "pending",
    });
    s = applyUpdate(s, {
      sessionUpdate: "tool_call_update",
      toolCallId: "t1",
      status: "completed",
      content: [{ type: "content", content: { type: "text", text: "hi" } }],
    });
    expect(s.items[0]).toMatchObject({ status: "completed", detail: "hi" });
    s = applyUpdate(s, {
      sessionUpdate: "tool_call_update",
      toolCallId: "t1",
      status: "failed",
      content: [{ type: "content", content: { type: "text", text: "nope" } }],
    });
    expect(s.items[0]).toMatchObject({ status: "failed", detail: "nope" });
  });

  test("usage / plan / mode / notice", () => {
    let s = emptyState();
    s = applyUpdate(s, {
      sessionUpdate: "usage",
      input: 10,
      output: 5,
      cum_usd: 0.01,
    });
    expect(s.usage.input).toBe(10);
    expect(s.items).toHaveLength(0);
    s = applyUpdate(s, { sessionUpdate: "plan", plan: "do stuff" });
    s = applyUpdate(s, { sessionUpdate: "mode", from: "plan", mode: "agent" });
    s = applyUpdate(s, { sessionUpdate: "notice", text: "⎇ subagent" });
    expect(s.items.map((i) => i.kind)).toEqual(["plan", "note", "note"]);
    expect((s.items[1] as any).text).toBe("mode: plan → agent");
  });

  test("unknown updates are ignored; state is not mutated", () => {
    const s = emptyState();
    const s2 = applyUpdate(s, { sessionUpdate: "???", x: 1 });
    expect(s2).toBe(s);
    expect(s.items).toHaveLength(0);
  });
});

describe("summarizeArg", () => {
  test("picks the salient field per tool", () => {
    expect(summarizeArg("bash", { command: "ls" })).toBe("ls");
    expect(summarizeArg("read", { path: "a.ts" })).toBe("a.ts");
    expect(summarizeArg("edit", { path: "b.ts", edits: [] })).toBe("b.ts");
    expect(summarizeArg("grep", { pattern: "foo" })).toBe("foo");
    expect(summarizeArg("write", {})).toBeUndefined();
    expect(summarizeArg("bash", undefined)).toBeUndefined();
  });
});

describe("streaming commits", () => {
  test("splits at the last paragraph break outside code fences", () => {
    expect(splitStreaming("one line, still going")).toEqual(["", "one line, still going"]);
    expect(splitStreaming("para one\n\npara two grows")).toEqual(["para one", "para two grows"]);
    // a blank line inside an open fence is not a safe cut
    const t = "intro\n\n```ts\nconst a = 1;\n\nconst b = 2;";
    expect(splitStreaming(t)).toEqual(["intro", "```ts\nconst a = 1;\n\nconst b = 2;"]);
    // after the fence closes, a later break is fine again
    const u = "```\nx\n\ny\n```\n\ntail";
    expect(splitStreaming(u)[0]).toBe("```\nx\n\ny\n```");
  });

  test("settle commits a prefix: pending tools hold back, streaming text gives up finished paragraphs", () => {
    const user = { kind: "user" as const, id: "u", text: "hi" };
    const pending = { kind: "tool" as const, id: "t", toolCallId: "c", name: "bash", status: "pending" as const };
    const after = { kind: "note" as const, id: "n", text: "later" };
    let r = settle([user, pending, after], true);
    expect(r.done.map((i) => i.id)).toEqual(["u"]);
    expect(r.live.map((i) => i.id)).toEqual(["t", "n"]);
    r = settle([{ ...pending, status: "completed" }, after], true);
    expect(r.live).toHaveLength(0);
    const reply = { kind: "assistant" as const, id: "a", text: "done para\n\nstill typ" };
    r = settle([reply], true);
    expect(r.done).toHaveLength(1);
    expect((r.done[0] as any).text).toBe("done para");
    expect(r.live).toEqual([{ ...reply, text: "still typ" }]);
    // once the turn ends the tail settles too
    r = settle(r.live, false);
    expect(r.live).toHaveLength(0);
  });

  test("errors land as error notes", () => {
    const s = applyUpdate(emptyState(), { sessionUpdate: "error", text: "provider 401: nope" });
    expect(s.items[0]).toMatchObject({ kind: "note", tone: "error", text: "provider 401: nope" });
  });
});
