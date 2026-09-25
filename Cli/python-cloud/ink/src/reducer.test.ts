import { describe, expect, test } from "bun:test";
import {
  applyUpdate,
  filterSlash,
  SLASH_COMMANDS,
  summarizeArg,
} from "./reducer";
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

describe("filterSlash", () => {
  test("prefix filter on the name without the slash", () => {
    expect(filterSlash("").length).toBe(SLASH_COMMANDS.length);
    expect(filterSlash("m").map((c) => c.name)).toEqual(["/model"]);
    expect(filterSlash("p").map((c) => c.name)).toEqual(["/plan"]);
    expect(filterSlash("zz")).toEqual([]);
    expect(filterSlash("MO").map((c) => c.name)).toEqual(["/model"]);
  });
});
