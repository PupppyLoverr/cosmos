// Contract §2 tools — identical semantics to ztools (Rust) / zdev tools.py.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

// tools.json is embedded verbatim so `tools dump` stays byte-identical to
// contracts/tools.json inside the compiled binary too.
// (`with { type: "text" }` gives the raw string at runtime; tsc still types
// the .json specifier as parsed JSON — hence the cast.)
import TOOLS_JSON_TEXT from "../tools.json" with { type: "text" };

const TOOLS_RAW = TOOLS_JSON_TEXT as unknown as string;

export const TOOLS_JSON = TOOLS_RAW;
export const RESULT_CAP = 30_000;
export const LARGE_READ_LINES = 200;
export const PLAN_TOOLS = new Set([
  "read",
  "grep",
  "glob",
  "ls",
  "memory_search",
  "submit_plan",
  "tool_search",
]);

export function toolsDump(): string {
  return TOOLS_JSON;
}

export function toolSpecs(): Record<string, any>[] {
  return JSON.parse(TOOLS_JSON);
}

export class Ctx {
  workspace: string;
  readState = new Map<string, string>();
  escalations: string[] = [];
  netAllow = false;
  noSandbox = false;
  sandboxDegraded: string | null = null;
  largeReadLines = LARGE_READ_LINES;
  mcpServers: Record<string, any> = {};

  constructor(workspace: string) {
    this.workspace = fs.realpathSync(workspace);
  }
}

export function capResult(s: string): string {
  if (s.length <= RESULT_CAP) return s;
  return `${s.slice(0, RESULT_CAP)}\n[truncated: ${s.length} chars > ${RESULT_CAP} cap]`;
}

// pathlib-Windows semantics: absolute needs a drive (C:\) or UNC (\\share).
function isAbsolute(p: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("\\\\") || p.startsWith("//");
}

export function resolve(ctx: Ctx, p: string): string {
  const joined = isAbsolute(p) ? p : path.join(ctx.workspace, p);
  const norm = path.normalize(joined);
  const root = path.normalize(ctx.workspace);
  if (!(norm === root || norm.startsWith(root + path.sep))) {
    throw new Error(`path escapes workspace: ${p}`);
  }
  return norm;
}

function sha6(line: string): string {
  return createHash("sha256")
    .update(line.replace(/\r?\n$/, ""), "utf8")
    .digest("hex")
    .slice(0, 6);
}

function readFile(p: string): [string, string] {
  const data = fs.readFileSync(p);
  return [data.toString("utf8"), createHash("sha256").update(data).digest("hex")];
}

function read(ctx: Ctx, inp: Record<string, any>): string {
  const p = resolve(ctx, inp.path ?? "");
  const [text, sha] = readFile(p);
  const lines = text.split("\n");
  const n =
    lines.length && lines[lines.length - 1] === "" && text.endsWith("\n")
      ? lines.length - 1
      : lines.length;
  const start = Math.max(1, Math.floor(inp.start ?? 1));
  const end = Math.min(n, Math.floor(inp.end ?? n));
  if (!("start" in inp) && !("end" in inp) && n > ctx.largeReadLines) {
    throw new Error(
      `large_read_guard: ${n} lines > ${ctx.largeReadLines} limit — read with ` +
        "start/end ranges (<=200 lines) or fetch the meridian cache.read summary",
    );
  }
  let out = `file_sha256=${sha} lines=${n}\n`;
  for (let i = start; i <= end; i++) {
    if (i < 1 || i > n) break;
    const line = lines[i - 1];
    out += `${i}:${sha6(line)}|${line}\n`;
  }
  ctx.readState.set(p, sha);
  return out.replace(/\n$/, "");
}

function parseAnchor(s: string): [number, string] {
  const i = s.indexOf(":");
  return [parseInt(s.slice(0, i), 10), s.slice(i + 1)];
}

function fresh(lines: string[], around: number, n: number): string {
  const lo = Math.max(1, around - 5);
  const hi = Math.min(n, around + 5);
  let out = "";
  for (let i = lo; i <= hi; i++) out += `${i}:${sha6(lines[i - 1])}|${lines[i - 1]}\n`;
  return out;
}

function edit(ctx: Ctx, inp: Record<string, any>): string {
  const p = resolve(ctx, inp.path ?? "");
  const [text, sha] = readFile(p);
  const want = inp.file_sha256;
  if (want && want !== sha) {
    throw new Error(`STALE: file_sha256 mismatch (want ${want}, have ${sha})`);
  }
  const newline = text.includes("\r\n") ? "\r\n" : "\n";
  const lines = text.split("\n");
  const n = lines.length;
  const pending: [number, number, string][] = [];
  let stale = "";
  for (const e of inp.edits ?? []) {
    const [sN, sH] = parseAnchor(e.start ?? "");
    const [eN, eH] = parseAnchor(e.end ?? "");
    const ok =
      sN >= 1 &&
      eN >= sN &&
      eN <= n &&
      sha6(lines[sN - 1]) === sH &&
      sha6(lines[eN - 1]) === eH;
    if (!ok) {
      stale += `anchor mismatch near line ${sN}..${eN}:\n${fresh(lines, sN, n)}\n`;
    } else {
      pending.push([sN, eN, e.new_text ?? ""]);
    }
  }
  if (stale) throw new Error(`STALE\n${stale}`);
  const owned = [...lines];
  for (const [sN, eN, newText] of pending.sort((a, b) => b[0] - a[0])) {
    const repl = newText.split("\n");
    if (repl.length && repl[repl.length - 1] === "" && newText) repl.pop();
    owned.splice(sN - 1, eN - (sN - 1), ...repl);
  }
  let body = owned.join(newline);
  if (text.endsWith(newline) && !body.endsWith(newline)) body += newline;
  atomicWrite(p, Buffer.from(body, "utf8"));
  return `ok ${p}`;
}

function write(ctx: Ctx, inp: Record<string, any>): string {
  const p = resolve(ctx, inp.path ?? "");
  const content = inp.content ?? "";
  if (fs.existsSync(p)) {
    const [, sha] = readFile(p);
    if (ctx.readState.get(p) !== sha) {
      throw new Error(
        `STALE: ${p} exists and was not read this session (or changed)`,
      );
    }
  }
  fs.mkdirSync(path.dirname(p), { recursive: true });
  atomicWrite(p, Buffer.from(content, "utf8"));
  const [, sha] = readFile(p);
  ctx.readState.set(p, sha);
  return `ok ${p}`;
}

function atomicWrite(p: string, data: Buffer): void {
  const tmp = `${p}.ztmp${process.pid}`;
  const fd = fs.openSync(tmp, "w");
  try {
    fs.writeSync(fd, data);
    fs.fsyncSync(fd);
  } finally {
    fs.closeSync(fd);
  }
  try {
    fs.renameSync(tmp, p);
  } catch {
    fs.writeFileSync(p, data);
    fs.unlinkSync(tmp);
  }
}

function ls(ctx: Ctx, inp: Record<string, any>): string {
  const p = resolve(ctx, inp.path ?? ".");
  const entries = fs
    .readdirSync(p, { withFileTypes: true })
    .map((e) => (e.isDirectory() ? e.name + "/" : e.name))
    .sort();
  return entries.join("\n");
}

// ── fnmatch: Python fnmatch on Windows is case-insensitive (normcase) ──────
function fnmatchRe(pat: string): RegExp {
  let re = "";
  let i = 0;
  while (i < pat.length) {
    const c = pat[i];
    if (c === "*") re += ".*";
    else if (c === "?") re += ".";
    else if (c === "[") {
      let j = i + 1;
      if (pat[j] === "!") j++;
      if (pat[j] === "]") j++;
      while (j < pat.length && pat[j] !== "]") j++;
      if (j >= pat.length) {
        re += "\\[";
      } else {
        let cls = pat.slice(i + 1, j);
        if (cls.startsWith("!")) cls = "^" + cls.slice(1);
        cls = cls.replace(/\\/g, "\\\\");
        re += `[${cls}]`;
        i = j;
      }
    } else {
      re += c.replace(/[.+^${}()|\\]/g, "\\$&");
    }
    i++;
  }
  return new RegExp(`^${re}$`, "i");
}

const fnmatch = (name: string, pat: string) => fnmatchRe(pat).test(name);

function gitignorePatterns(root: string): string[] {
  const pats: string[] = [];
  const gi = path.join(root, ".gitignore");
  if (fs.existsSync(gi) && fs.statSync(gi).isFile()) {
    for (const raw of fs.readFileSync(gi, "utf8").split(/\r?\n/)) {
      const line = raw.trim();
      if (line && !line.startsWith("#")) pats.push(line.replace(/^\/+/, ""));
    }
  }
  return pats;
}

function ignored(rel: string, pats: string[]): boolean {
  rel = rel.replace(/\\/g, "/");
  for (const p of pats) {
    if (p.endsWith("/")) {
      const base = p.slice(0, -1);
      if (rel.startsWith(base) || `/${rel}/`.includes(`/${base}/`)) return true;
    } else if (
      fnmatch(rel, p) ||
      fnmatch(path.basename(rel.replace(/\//g, path.sep)), p)
    ) {
      return true;
    }
  }
  return false;
}

function* walkFiles(ctx: Ctx, root: string): Generator<[string, string]> {
  const pats = gitignorePatterns(ctx.workspace);
  const stack = [root];
  while (stack.length) {
    const dir = stack.pop()!;
    let ents: fs.Dirent[];
    try {
      ents = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const e of ents) {
      const full = path.join(dir, e.name);
      const rel = path.relative(ctx.workspace, full).replace(/\\/g, "/");
      if (e.isDirectory()) {
        if (!ignored(rel, pats)) stack.push(full);
      } else if (!ignored(rel, pats)) {
        yield [full, rel];
      }
    }
  }
}

function glob(ctx: Ctx, inp: Record<string, any>): string {
  const pattern = inp.pattern ?? "*";
  const out: string[] = [];
  for (const [, rel] of walkFiles(ctx, ctx.workspace)) {
    if (fnmatch(rel, pattern)) out.push(rel);
    if (out.length >= 500) break;
  }
  return out.sort().join("\n");
}

function grep(ctx: Ctx, inp: Record<string, any>): string {
  let rePat: RegExp;
  try {
    rePat = new RegExp(inp.pattern ?? "");
  } catch (e) {
    throw new Error(`bad regex: ${e}`);
  }
  const root = inp.path ? resolve(ctx, inp.path) : ctx.workspace;
  const globPat = inp.glob;
  const out: string[] = [];
  for (const [full, rel] of walkFiles(ctx, root)) {
    if (globPat && !fnmatch(rel, globPat)) continue;
    let text: string;
    try {
      text = fs.readFileSync(full, "utf8");
    } catch {
      continue;
    }
    const lines = text.split(/\r?\n/);
    for (let i = 0; i < lines.length; i++) {
      if (rePat.test(lines[i])) {
        out.push(`${rel}:${i + 1}:${lines[i]}`);
        if (out.length >= 200) return out.join("\n") + "\n[cap: 200 matches]";
      }
    }
  }
  return out.join("\n");
}

function findBin(names: string[], env: string): string | null {
  const v = process.env[env];
  if (v && fs.existsSync(v) && fs.statSync(v).isFile()) return v;
  for (const d of (process.env.PATH ?? "").split(path.delimiter)) {
    for (const n of names) {
      const c = path.join(d, n);
      if (fs.existsSync(c) && fs.statSync(c).isFile()) return c;
    }
  }
  // exe dir: the compiled binary's directory (…/bin holds the toolchain),
  // then the Python-harness convention dirname(COSMOS_BIN_DIR).
  const dirs = [
    path.dirname(process.execPath),
    path.dirname(process.env.COSMOS_BIN_DIR ?? ""),
  ];
  for (const d of dirs) {
    for (const n of names) {
      const c = path.join(d, n);
      if (fs.existsSync(c) && fs.statSync(c).isFile()) return c;
    }
  }
  return null;
}

function shellArgv(command: string): string[] {
  for (const d of (process.env.PATH ?? "").split(path.delimiter)) {
    const b = path.join(d, "bash.exe");
    if (fs.existsSync(b) && fs.statSync(b).isFile()) return [b, "-lc", command];
  }
  return [
    "powershell.exe",
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    command,
  ];
}

function bash(ctx: Ctx, inp: Record<string, any>): string {
  const command = inp.command ?? "";
  const timeout = Math.min(Math.floor(inp.timeout_s ?? 120), 600);
  const argv = shellArgv(command);
  if (ctx.noSandbox) {
    const out = spawnSync(argv[0], argv.slice(1), {
      cwd: ctx.workspace,
      timeout: timeout * 1000,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      windowsHide: true,
    });
    return (out.stdout ?? "") + (out.stderr ?? "");
  }
  const zsb = findBin(["zsandbox.exe", "zsandbox"], "ZSANDBOX_BIN");
  if (!zsb) return "ERROR: zsandbox.exe not found (set ZSANDBOX_BIN)";
  const note = ctx.sandboxDegraded
    ? `[sandbox:degraded: ${ctx.sandboxDegraded}]\n`
    : "";
  try {
    const out = spawnSync(
      zsb,
      [
        "exec",
        "--workspace",
        ctx.workspace,
        "--net",
        ctx.netAllow ? "allow" : "deny",
        "--timeout",
        String(timeout),
        "--",
        ...argv,
      ],
      {
        cwd: ctx.workspace,
        timeout: (timeout + 30) * 1000,
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
        windowsHide: true,
      },
    );
    if (out.error && (out.error as any).code === "ETIMEDOUT") {
      return note + "[timeout]";
    }
    let s = note + (out.stdout ?? "") + (out.stderr ?? "");
    if (out.status === 124) s += "\n[timeout]";
    return s;
  } catch {
    return note + "[timeout]";
  }
}

function meridian(ctx: Ctx, args: string[]): string {
  const mer = findBin(["meridian.exe", "meridian"], "MERIDIAN_BIN");
  if (!mer) {
    throw new Error("meridian bridge binary not found (set MERIDIAN_BIN)");
  }
  const out = spawnSync(mer, args, {
    encoding: "utf8",
    env: process.env,
    windowsHide: true,
  });
  if (out.status !== 0) {
    throw new Error(`meridian ${args[0]}: ${(out.stderr ?? "").trim()}`);
  }
  return (out.stdout ?? "").trim();
}

function memorySearch(ctx: Ctx, inp: Record<string, any>): string {
  return meridian(ctx, [
    "search",
    "--query",
    inp.query ?? "",
    "--k",
    String(inp.k ?? 5),
  ]);
}

function memoryWrite(ctx: Ctx, inp: Record<string, any>): string {
  const ty = inp.type ?? "";
  if (!ty) {
    return "ERROR: missing required field: type (decision|preference|map|correction|playbook)";
  }
  return meridian(ctx, [
    "write",
    "--type",
    ty,
    "--scope",
    inp.scope ?? "global",
    "--text",
    inp.text ?? "",
  ]);
}

function mcpOrSearch(ctx: Ctx, _name: string, _inp: Record<string, any>): string {
  if (!Object.keys(ctx.mcpServers).length) {
    return "ERROR: no MCP servers configured";
  }
  return "ERROR: mcp client not configured for this harness run";
}

const DISPATCH: Record<
  string,
  (ctx: Ctx, inp: Record<string, any>) => string
> = {
  read,
  edit,
  write,
  ls,
  glob,
  grep,
  bash,
  memory_search: memorySearch,
  memory_write: memoryWrite,
};

export function run(ctx: Ctx, name: string, inp: Record<string, any>): string {
  try {
    const fn = DISPATCH[name];
    if (fn) return capResult(fn(ctx, inp));
    if (name === "tool_search" || name === "mcp_call") {
      return capResult(mcpOrSearch(ctx, name, inp));
    }
    if (name === "submit_plan" || name === "task" || name === "request_escalation") {
      throw new Error(`${name} is handled by the harness, not the tool layer`);
    }
    throw new Error(`unknown tool: ${name}`);
  } catch (e: any) {
    return `ERROR: ${e?.message ?? e}`;
  }
}
