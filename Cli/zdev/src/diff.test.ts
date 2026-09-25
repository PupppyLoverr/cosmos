import { describe, expect, test } from "bun:test";
import {
  buildEditDiff,
  buildWriteDiff,
  diffLines,
  parseReadDetail,
} from "./diff";
import { toolLabel } from "./reducer";

describe("parseReadDetail", () => {
  test("parses n:sha|line rows, skips header", () => {
    const d = parseReadDetail(
      "file_sha256=abc lines=3\n1:aaaaaa|one\n2:bbbbbb|two words\n3:cccccc|three",
    );
    expect(d.get(2)).toBe("two words");
    expect(d.size).toBe(3);
  });
  test("empty/garbage input → empty map", () => {
    expect(parseReadDetail("").size).toBe(0);
    expect(parseReadDetail("no anchors here").size).toBe(0);
  });
});

describe("diffLines", () => {
  test("replace one line", () => {
    const d = diffLines(["a", "b", "c"], ["a", "B", "c"], 1);
    expect(d).toEqual([
      { type: "ctx", n: 1, text: "a" },
      { type: "del", n: 2, text: "b" },
      { type: "add", n: 2, text: "B" },
      { type: "ctx", n: 3, text: "c" },
    ]);
  });
  test("pure insertion + deletion", () => {
    const d = diffLines(["a"], ["a", "x", "y"], 1);
    expect(d.map((l) => l.type)).toEqual(["ctx", "add", "add"]);
    const d2 = diffLines(["a", "x", "y"], ["a"], 1);
    expect(d2.map((l) => l.type)).toEqual(["ctx", "del", "del"]);
  });
});

describe("buildEditDiff", () => {
  const reads = new Map([
    [
      "f.ts",
      new Map([
        [1, "const a = 1"],
        [2, "const b = 2"],
        [3, "const c = 3"],
      ]),
    ],
  ]);
  test("real del/add rows from the read cache", () => {
    const hunks = buildEditDiff(
      {
        path: "f.ts",
        edits: [{ start: "2:bbbbbb", end: "2:bbbbbb", new_text: "const b = 20" }],
      },
      reads,
    );
    expect(hunks).toHaveLength(1);
    expect(hunks[0].header).toBe("@@ 2 @@");
    const types = hunks[0].lines.map((l) => l.type);
    expect(types).toContain("del");
    expect(types).toContain("add");
    expect(types).toContain("ctx"); // surrounding rows pulled in
    expect(hunks[0].lines.find((l) => l.type === "del")?.text).toBe(
      "const b = 2",
    );
  });
  test("multi-line range anchor", () => {
    const hunks = buildEditDiff(
      {
        path: "f.ts",
        edits: [{ start: "1:a", end: "2:b", new_text: "const z = 0" }],
      },
      reads,
    );
    expect(hunks[0].header).toBe("@@ 1–2 @@");
    expect(hunks[0].lines.filter((l) => l.type === "del")).toHaveLength(2);
  });
  test("unknown file → add-only fallback", () => {
    const hunks = buildEditDiff(
      { path: "nope.ts", edits: [{ start: "1:x", end: "1:x", new_text: "hi" }] },
      reads,
    );
    expect(hunks[0].lines).toEqual([{ type: "add", n: 1, text: "hi" }]);
  });
});

describe("buildWriteDiff", () => {
  test("numbered add rows, trailing newline trimmed", () => {
    const d = buildWriteDiff("one\ntwo\n");
    expect(d).toEqual([
      { type: "add", n: 1, text: "one" },
      { type: "add", n: 2, text: "two" },
    ]);
  });
});

describe("toolLabel", () => {
  test("claude display names", () => {
    expect(toolLabel("edit")).toBe("Update");
    expect(toolLabel("read")).toBe("Read");
    expect(toolLabel("bash")).toBe("Bash");
    expect(toolLabel("mcp_call")).toBe("MCP");
    expect(toolLabel("unknown_thing")).toBe("unknown_thing");
  });
});
