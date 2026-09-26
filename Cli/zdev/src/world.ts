// Per-session world.md. Same semantics as zcore/src/world.rs.

import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { cliHome } from "./proto";

export type World = {
  goal: string;
  status: string;
  decisions: string[];
  files: [string, string][];
  tests: [string, string][];
  open_questions: string[];
  plan: string;
  next: string;
  focus: string;
};

export function empty(): World {
  return {
    goal: "",
    status: "",
    decisions: [],
    files: [],
    tests: [],
    open_questions: [],
    plan: "",
    next: "",
    focus: "",
  };
}

export function worldPath(session: string, home = cliHome()): string {
  return path.join(home, "world", `${session}.md`);
}

export function sha8(bytes: Buffer | string): string {
  return createHash("sha256").update(bytes).digest("hex").slice(0, 8);
}

export function render(w: World): string {
  const files = w.files.map(([p, h]) => (h ? `${p} (${h})` : p)).join(", ");
  const tests = w.tests.map(([c, r]) => (r ? `${c} → ${r}` : c)).join("; ");
  return `# World\n${w.focus ? `focus: ${w.focus}\n` : ""}goal: ${w.goal}\nstatus: ${w.status}\ndecisions: ${w.decisions.join("; ")}\nfiles_touched: ${files}\ntests: ${tests}\nopen_questions: ${w.open_questions.join("; ")}\n# Plan\n${w.plan}\n# Next\n${w.next}\n`;
}

export function parse(text: string): World {
  const w = empty();
  let section = "";
  for (const line of text.split(/\r?\n/)) {
    if (line === "# World") section = "world";
    else if (line === "# Plan") section = "plan";
    else if (line === "# Next") section = "next";
    else if (line.startsWith("goal: ")) w.goal = line.slice(6);
    else if (line.startsWith("status: ")) w.status = line.slice(8);
    else if (line.startsWith("focus: ")) w.focus = line.slice(7);
    else if (section === "plan" && line) w.plan += (w.plan ? "\n" : "") + line;
    else if (section === "next" && line) w.next += (w.next ? "\n" : "") + line;
  }
  return w;
}

export function load(session: string, home = cliHome()): World {
  const p = worldPath(session, home);
  try {
    return parse(fs.readFileSync(p, "utf8"));
  } catch {
    return empty();
  }
}

export function save(session: string, w: World, home = cliHome()): void {
  const p = worldPath(session, home);
  fs.mkdirSync(path.dirname(p), { recursive: true });
  fs.writeFileSync(p, render(w));
}

export function touchFile(w: World, file: string, bytes: Buffer | string): void {
  const h = sha8(bytes);
  const i = w.files.findIndex(([p]) => p === file);
  if (i >= 0) w.files[i][1] = h;
  else w.files.push([file, h]);
  w.status = `touched ${file}`;
}

export function setGoalIfEmpty(w: World, text: string): void {
  if (!w.goal && !text.startsWith("[") && !text.startsWith("/")) {
    w.goal = text.slice(0, 400);
  }
}

export function applyCompactMd(w: World, md: string): void {
  const parsed = parse(md);
  if (parsed.goal) w.goal = parsed.goal;
  if (parsed.status) w.status = parsed.status;
  if (parsed.files.length) w.files = parsed.files;
  if (parsed.plan) w.plan = parsed.plan;
  if (parsed.next) w.next = parsed.next;
  if (parsed.focus) w.focus = parsed.focus;
}
