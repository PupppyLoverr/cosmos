// Input parsing, line editing and the composer menus.
import { describe, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { KeyParser, lineEnd, lineStart, verticalMove, wordLeft, wordRight } from "./keys";
import { describe as describeCmd, expandMentions, fuzzyScore, loadCommands, mentionAt, parseSlash, rank } from "./menus";

const names = (chunk: string) => new KeyParser().feed(chunk).map((k) => (k.name === "text" || k.name === "paste" ? `${k.meta ? "M-" : ""}${k.name}:${k.text}` : `${k.ctrl ? "C-" : ""}${k.meta ? "M-" : ""}${k.shift ? "S-" : ""}${k.name}`));

describe("KeyParser", () => {
  test("one chunk with several keys splits into each of them", () => {
    expect(names("\x1b[A\x1b[A\x1b[B")).toEqual(["up", "up", "down"]);
    expect(names("ab\r")).toEqual(["text:ab", "return"]);
    expect(names("\x7f\x7f")).toEqual(["backspace", "backspace"]);
  });

  test("modifiers, home/end, shift+tab, ctrl keys, ctrl+space, alt", () => {
    expect(names("\x1b[1;5D\x1b[1;3C\x1b[H\x1b[F\x1b[Z\x1b[3~")).toEqual(["C-left", "M-right", "home", "end", "S-tab", "delete"]);
    expect(names("\x03\x0f\x00\x1bx\x1b")).toEqual(["C-c", "C-o", "C-space", "M-text:x", "escape"]);
    expect(names("\n")).toEqual(["C-j"]);
  });

  test("bracketed paste — even split across chunks — is one paste", () => {
    const p = new KeyParser();
    const a = p.feed("\x1b[200~line one\rline");
    expect(a).toEqual([]);
    const b = p.feed(" two\x1b[201~x");
    expect(b.map((k) => [k.name, k.text])).toEqual([
      ["paste", "line one\rline two"],
      ["text", "x"],
    ]);
  });

  test("without bracketed paste, text with inner line breaks is a paste, not enter", () => {
    expect(names("first\rsecond")).toEqual(["paste:first\rsecond"]);
    expect(names("typed fast\r")).toEqual(["text:typed fast", "return"]);
  });
});

describe("line editing", () => {
  test("words, lines and vertical moves", () => {
    const s = "hello big world";
    expect(wordLeft(s, s.length)).toBe(10);
    expect(wordRight(s, 0)).toBe(5);
    const m = "abc\ndefgh\nij";
    expect(lineStart(m, 6)).toBe(4);
    expect(lineEnd(m, 6)).toBe(9);
    expect(verticalMove(m, 6, -1)).toBe(2);
    expect(verticalMove(m, 8, 1)).toBe(12);
    expect(verticalMove(m, 1, -1)).toBeNull();
  });
});

describe("menus", () => {
  test("fuzzy ranking favours prefixes and word starts", () => {
    const items = ["model", "mode", "context", "connect", "copy"];
    expect(items[rank(items, "mo", (x) => x)[0]]).toBe("mode");
    expect(items[rank(items, "ctx", (x) => x)[0]]).toBe("context");
    expect(rank(items, "zzz", (x) => x)).toEqual([]);
    expect(fuzzyScore("src/ui/app.tsx", "uiapp")).not.toBeNull();
    const files = ["src/components/Composer.tsx", "src/ui/app.tsx", "README.md"];
    expect(files[rank(files, "app", (x) => x)[0]]).toBe("src/ui/app.tsx");
  });

  test("slash parsing, custom commands and front matter", () => {
    expect(parseSlash("/mode plan")).toEqual(["mode", "plan"]);
    expect(parseSlash("/model")).toEqual(["model", ""]);
    expect(parseSlash("hi")).toBeNull();
    const ws = fs.mkdtempSync(path.join(os.tmpdir(), "zdev-cmd-"));
    fs.mkdirSync(path.join(ws, ".claude", "commands"), { recursive: true });
    fs.writeFileSync(path.join(ws, ".claude", "commands", "ship.md"), "---\ndescription: Tag and release\n---\nRelease $ARGUMENTS now");
    fs.writeFileSync(path.join(ws, ".claude", "commands", "model.md"), "shadowed");
    const cmds = loadCommands(ws);
    const ship = cmds.find((c) => c.name === "ship")!;
    expect(ship).toMatchObject({ desc: "Tag and release", hint: "[args]", body: "Release $ARGUMENTS now" });
    expect(cmds.filter((c) => c.name === "model").every((c) => !c.body)).toBe(true);
    expect(describeCmd("# Title line\nbody").desc).toBe("Title line");
    fs.rmSync(ws, { recursive: true, force: true });
  });

  test("@mentions: the token at the cursor, and inlining real files only", () => {
    expect(mentionAt("look at @src/u", 14)).toEqual({ start: 8, query: "src/u" });
    expect(mentionAt("mail me@x.com", 13)).toBeNull();
    const ws = fs.mkdtempSync(path.join(os.tmpdir(), "zdev-at-"));
    fs.writeFileSync(path.join(ws, "a.txt"), "alpha");
    const out = expandMentions("read @a.txt and @missing.txt", ws);
    expect(out).toContain('<file path="a.txt">\nalpha\n</file>');
    expect(out).not.toContain("missing.txt\">");
    expect(expandMentions("escape @../x", ws)).toBe("escape @../x");
    fs.rmSync(ws, { recursive: true, force: true });
  });
});
