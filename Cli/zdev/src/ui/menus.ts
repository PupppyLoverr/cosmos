// Menus behind the composer: fuzzy ranking, slash commands (built-in plus
// markdown custom commands, Claude-Code style), the `@file` index and the
// persisted prompt history. Pure where possible so `bun test` covers them.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { cliHome } from "../proto";

// ── fuzzy ranking ───────────────────────────────────────────────────────────

/** Subsequence score (higher is better), null when `q` doesn't match. Word
 * starts, consecutive runs and early matches score higher — the same feel as
 * the nucleo-backed menus in zcli. */
export function fuzzyScore(text: string, q: string): number | null {
  if (!q) return 0;
  const t = text.toLowerCase();
  const s = q.toLowerCase();
  let score = 0;
  let ti = 0;
  let prev = -2;
  for (const ch of s) {
    if (ch === " ") continue;
    const at = t.indexOf(ch, ti);
    if (at < 0) return null;
    const boundary = at === 0 || /[\s/_.\-]/.test(t[at - 1]);
    score += 10 + (boundary ? 8 : 0) + (at === prev + 1 ? 6 : 0) - Math.min(at - ti, 8);
    prev = at;
    ti = at + 1;
  }
  if (t.startsWith(s)) score += 30;
  else if (t.includes(s)) score += 15;
  return score - text.length * 0.05;
}

/** Indices of `items` ranked against `q` (original order for an empty q). */
export function rank<T>(items: T[], q: string, key: (t: T) => string): number[] {
  q = q.trim();
  if (!q) return items.map((_, i) => i);
  return items
    .map((it, i) => [fuzzyScore(key(it), q), i] as const)
    .filter((x): x is readonly [number, number] => x[0] !== null)
    .sort((a, b) => b[0] - a[0] || a[1] - b[1])
    .map((x) => x[1]);
}

// ── slash commands ──────────────────────────────────────────────────────────

export type Slash = { name: string; hint: string; desc: string; body?: string };

export const BUILTIN: Slash[] = [
  { name: "model", hint: "[provider/model]", desc: "Choose a model — live lists from every provider you've connected" },
  { name: "connect", hint: "[provider]", desc: "Add an API key: OpenAI, Anthropic, Google, OpenRouter, 9Router, …" },
  { name: "login", hint: "", desc: "Sign in with your ChatGPT Plus/Pro plan" },
  { name: "logout", hint: "[provider]", desc: "Remove a stored key or the ChatGPT sign-in" },
  { name: "mode", hint: "[plan|agent|yolo]", desc: "Show or switch mode (shift+tab cycles)" },
  { name: "plan", hint: "", desc: "Plan mode — read-only, ends in an approvable plan" },
  { name: "approve", hint: "", desc: "Approve the submitted plan and run it" },
  { name: "usage", hint: "", desc: "Tokens, cost and ChatGPT plan limits for this session" },
  { name: "context", hint: "", desc: "How full the context window is" },
  { name: "resume", hint: "", desc: "Continue an earlier session" },
  { name: "init", hint: "", desc: "Write an AGENTS.md that explains this repo to agents" },
  { name: "review", hint: "[focus]", desc: "Review the uncommitted changes" },
  { name: "diff", hint: "", desc: "Show what changed in the working tree" },
  { name: "copy", hint: "", desc: "Copy the last reply to the clipboard" },
  { name: "voice", hint: "", desc: "Dictate your task (ctrl+space)" },
  { name: "session", hint: "", desc: "Show the session id and how to resume it" },
  { name: "clear", hint: "", desc: "Clear the screen and start a fresh session" },
  { name: "onboarding", hint: "", desc: "Replay the first-run setup" },
  { name: "help", hint: "", desc: "Keys and commands" },
  { name: "quit", hint: "", desc: "Exit zdev" },
];

/** Front matter `description:` or the first line → menu text; rest → body. */
export function describe(text: string): { desc: string; body: string } {
  if (text.startsWith("---")) {
    const end = text.indexOf("\n---", 3);
    if (end > 0) {
      const fm = text.slice(3, end);
      const desc = (fm.split(/\r?\n/).find((l) => l.startsWith("description:")) ?? "")
        .replace(/^description:/, "")
        .trim()
        .replace(/^"|"$/g, "");
      return { desc, body: text.slice(end + 4).replace(/^[-\r\n]+/, "") };
    }
  }
  const first = (text.split(/\r?\n/).find((l) => l.trim()) ?? "").replace(/^#+/, "").trim();
  return { desc: first.slice(0, 80), body: text };
}

function scan(dir: string, out: Slash[]) {
  let files: string[];
  try {
    files = fs.readdirSync(dir).filter((f) => f.endsWith(".md")).sort();
  } catch {
    return;
  }
  for (const f of files) {
    const name = f.slice(0, -3);
    if (out.some((s) => s.name === name)) continue; // built-ins and earlier dirs win
    try {
      const { desc, body } = describe(fs.readFileSync(path.join(dir, f), "utf8"));
      out.push({ name, hint: body.includes("$ARGUMENTS") ? "[args]" : "", desc: desc || "Custom command", body });
    } catch {}
  }
}

/** Built-ins plus markdown commands: `.zcli/commands` (project),
 * `~/.cosmos-cli/commands` (personal), `.claude/commands` (Claude Code's). */
export function loadCommands(cwd: string): Slash[] {
  const all = [...BUILTIN];
  scan(path.join(cwd, ".zcli", "commands"), all);
  scan(path.join(cliHome(), "commands"), all);
  scan(path.join(cwd, ".claude", "commands"), all);
  return all;
}

/** `/name args` → [name, args]; null when the input isn't a command. */
export function parseSlash(input: string): [string, string] | null {
  const m = input.match(/^\/(\S*)(?:\s+([\s\S]*))?$/);
  return m ? [m[1], (m[2] ?? "").trim()] : null;
}

// ── @file index ─────────────────────────────────────────────────────────────

const SKIP_DIRS = new Set([".git", "node_modules", "target", "dist", "build", ".venv", "__pycache__", ".next", ".cache"]);

/** Workspace files for `@` mentions: `git ls-files` (tracked + untracked,
 * gitignore respected) or a capped directory walk outside a repo. */
export function fileIndex(cwd: string, cap = 20_000): string[] {
  const git = spawnSync("git", ["ls-files", "--cached", "--others", "--exclude-standard"], {
    cwd,
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (git.status === 0 && git.stdout) return git.stdout.split(/\r?\n/).filter(Boolean).slice(0, cap);
  const out: string[] = [];
  const stack = [""];
  while (stack.length && out.length < cap) {
    const rel = stack.pop()!;
    let ents: fs.Dirent[];
    try {
      ents = fs.readdirSync(path.join(cwd, rel), { withFileTypes: true });
    } catch {
      continue;
    }
    for (const e of ents) {
      const r = rel ? `${rel}/${e.name}` : e.name;
      if (e.isDirectory()) {
        if (!SKIP_DIRS.has(e.name) && !e.name.startsWith(".")) stack.push(r);
      } else out.push(r);
    }
  }
  return out;
}

/** The `@partial` token ending at `cur`, if the cursor sits in one. */
export function mentionAt(buf: string, cur: number): { start: number; query: string } | null {
  const before = buf.slice(0, cur);
  const m = before.match(/(^|\s)@([^\s@]*)$/);
  if (!m) return null;
  return { start: cur - m[2].length - 1, query: m[2] };
}

/** Inline each `@path` that names a workspace file as a context block. */
export function expandMentions(text: string, cwd: string): string {
  let embedded = "";
  const root = path.resolve(cwd);
  const seen = new Set<string>();
  for (const m of text.matchAll(/(?:^|\s)@([^\s@]+)/g)) {
    const rel = m[1].replace(/[.,;:!?)]+$/, "");
    if (seen.has(rel)) continue;
    seen.add(rel);
    const p = path.resolve(root, rel);
    if (!p.startsWith(root)) continue;
    try {
      if (!fs.statSync(p).isFile()) continue;
      const body = fs.readFileSync(p, "utf8");
      const lines = body.split("\n");
      const shown = lines.length > 400 ? lines.slice(0, 400).join("\n") + `\n… (${lines.length - 400} more lines)` : body;
      embedded += `\n\n<file path="${rel}">\n${shown}\n</file>`;
    } catch {}
  }
  return text + embedded;
}

// ── prompt history ──────────────────────────────────────────────────────────

const historyPath = () => path.join(cliHome(), "zdev_history.jsonl");

export function loadHistory(limit = 500): string[] {
  try {
    return fs
      .readFileSync(historyPath(), "utf8")
      .split(/\r?\n/)
      .filter(Boolean)
      .map((l) => {
        try {
          return String(JSON.parse(l).text ?? "");
        } catch {
          return "";
        }
      })
      .filter(Boolean)
      .slice(-limit);
  } catch {
    return [];
  }
}

export function appendHistory(text: string): void {
  try {
    fs.mkdirSync(cliHome(), { recursive: true });
    fs.appendFileSync(historyPath(), JSON.stringify({ text, ts: Date.now() }) + "\n", "utf8");
  } catch {}
}

// ── clipboard ───────────────────────────────────────────────────────────────

/** Copy via OSC 52 (works over SSH and in Windows Terminal) and, on Windows,
 * the system clipboard too. */
export function copyText(text: string): void {
  const b64 = Buffer.from(text, "utf8").toString("base64");
  try {
    process.stdout.write(`\x1b]52;c;${b64}\x07`);
  } catch {}
  // clip.exe mangles non-ASCII; decode base64 inside PowerShell instead
  if (process.platform === "win32" && b64.length < 24_000) {
    try {
      const { spawn } = require("node:child_process") as typeof import("node:child_process");
      spawn(
        "powershell",
        ["-NoProfile", "-NonInteractive", "-Command", `Set-Clipboard -Value ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${b64}')))`],
        { windowsHide: true, stdio: "ignore", detached: true },
      ).unref();
    } catch {}
  }
}
