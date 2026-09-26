// Five-layer compaction. Same order as zcore/src/compact.rs.
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { cliHome } from "./proto";
import type { Message } from "./store";

export const THRASH_MIN_REDUCTION = 0.2;
export const SNIP_AFTER = 48;
export const SNIP_KEEP_HEAD = 3;
export const SNIP_KEEP_NEWEST = 40;
export const MICRO_KEEP = 3;

export function prefixHash(system: string[], toolsJson: string): string {
  const h = createHash("sha256");
  for (const s of system) {
    h.update(s);
    h.update("\0");
  }
  h.update(toolsJson);
  return h.digest("hex");
}

export function estimateTokens(messages: Message[], boot: number): number {
  let n = boot;
  for (const m of messages) {
    for (const b of m.content) {
      n += Math.floor(String((b as any).text ?? (b as any).content ?? "").length / 4);
    }
  }
  return n;
}

export function cacheWarm(
  lastMs: number | null,
  prefixNow: string,
  prefixThen: string,
  nowMs: number,
  warmMs = 240_000,
): boolean {
  return lastMs != null && nowMs - lastMs < warmMs && prefixNow === prefixThen;
}

function persist(home: string, content: string): { path: string; sha: string; bytes: number; lines: number } {
  const sha = createHash("sha256").update(content).digest("hex");
  const dir = path.join(home, "cache", "tool");
  fs.mkdirSync(dir, { recursive: true });
  const p = path.join(dir, `${sha}.txt`);
  if (!fs.existsSync(p)) fs.writeFileSync(p, content);
  return { path: p, sha, bytes: content.length, lines: content.split("\n").length };
}

function middleOut(content: string): string {
  const lines = content.split("\n");
  if (lines.length <= 48) return content;
  const omitted = content.length - lines.slice(0, 24).join("\n").length - lines.slice(-24).join("\n").length;
  return `${lines.slice(0, 24).join("\n")}\n… ${omitted} chars omitted …\n${lines.slice(-24).join("\n")}`;
}

export function layer1(messages: Message[], home: string, largeReadLines: number): void {
  const cap = Math.max(largeReadLines * 40, 8000);
  for (const m of messages) {
    for (const b of m.content) {
      if (b.type !== "tool_result") continue;
      const c = String(b.content ?? "");
      if (c.length > cap || c.split("\n").length > largeReadLines) {
        const t = persist(home, c);
        (b as any).content = `[tool_result persisted path=${t.path} sha256=${t.sha} bytes=${t.bytes} lines=${t.lines}]\n${middleOut(c)}\n`;
      }
    }
  }
}

export function layer2(messages: Message[]): { kind: string; data: any } | null {
  if (messages.length <= SNIP_AFTER) return null;
  const head = SNIP_KEEP_HEAD;
  const tail = messages.length - SNIP_KEEP_NEWEST;
  if (tail <= head) return null;
  const dropped = tail - head;
  const next = [
    ...messages.slice(0, head),
    { role: "user" as const, content: [{ type: "text" as const, text: `[snipped ${dropped} messages; full log in session]` }] },
    ...messages.slice(tail),
  ];
  messages.splice(0, messages.length, ...next);
  return { kind: "snip", data: { dropped } };
}

export function layer3(messages: Message[], warm: boolean): void {
  const start = warm ? Math.max(0, messages.length - MICRO_KEEP) : 0;
  const protect = messages.length - MICRO_KEEP;
  messages.forEach((m, i) => {
    if (i < start || i >= protect) return;
    for (const b of m.content) {
      if (b.type !== "tool_result") continue;
      const c = String(b.content ?? "");
      if (c.startsWith("[cleared") || c.startsWith("[tool_result persisted")) continue;
      (b as any).content = `[cleared tool_result id=${(b as any).tool_use_id ?? ""} path=]`;
    }
  });
}

export function layer4(messages: Message[]): Message[] {
  const out: Message[] = [];
  let i = 0;
  while (i < messages.length) {
    const paths: string[] = [];
    let j = i;
    while (j < messages.length) {
      const reads = messages[j].content.filter((b) => b.type === "tool_use" && (b as any).name === "read");
      if (!reads.length) break;
      for (const r of reads) {
        const p = (r as any).input?.path;
        if (p) paths.push(p);
      }
      j++;
    }
    if (paths.length >= 2) {
      out.push({ role: "user", content: [{ type: "text", text: `[${paths.length} reads: ${paths.join(", ")}]` }] });
      i = j;
    } else {
      out.push(messages[i]);
      i++;
    }
  }
  return out;
}

export function summarize(messages: Message[], focus?: string): string {
  let goal = "";
  const files: string[] = [];
  let last = "";
  for (const m of messages) {
    for (const b of m.content) {
      if (b.type === "text" && m.role === "user" && !goal && !(b as any).text?.startsWith("[")) {
        goal = String((b as any).text).slice(0, 400);
      }
      if (b.type === "text" && m.role === "assistant") last = String((b as any).text).slice(0, 400);
      if (b.type === "tool_use" && (b as any).input?.path) files.push((b as any).input.path);
    }
  }
  return `# World\n${focus ? `focus: ${focus}\n` : ""}goal: ${goal}\nstatus: working\ndecisions: \nfiles_touched: ${files.join(", ")}\ntests: \nopen_questions: \n# Plan\n\n# Next\n${last}\n`;
}

export function layer5(messages: Message[], keepTail: number, focus?: string): { world: string; ev: any } {
  const world = summarize(messages, focus);
  const tail = messages.slice(-keepTail);
  messages.splice(0, messages.length, { role: "user", content: [{ type: "text", text: `[compact world]\n${world}` }] }, ...tail);
  return { world, ev: { kind: "compact", data: { reason: "l5", focus } } };
}

export function shouldL5(tokens: number, context: number, reserved: number, warm: boolean, ratio = 0.8): boolean {
  if (!context) return false;
  const thresh = Math.min(context * ratio, context - reserved - 8000);
  if (tokens >= thresh) return true;
  return !warm && tokens >= context / 2;
}

export function compact(
  messages: Message[],
  session: string,
  opts: {
    home?: string;
    largeReadLines?: number;
    context?: number;
    boot?: number;
    warm?: boolean;
    force?: boolean;
    focus?: string;
    keepTail?: number;
    reserved?: number;
    lastL5Before?: number | null;
    lastL5After?: number | null;
  },
): { events: { kind: string; data: any }[]; world?: string; before: number; after: number } {
  const home = opts.home ?? cliHome();
  const before = estimateTokens(messages, opts.boot ?? 0);
  const events: { kind: string; data: any }[] = [];
  layer1(messages, home, opts.largeReadLines ?? 200);
  const snip = layer2(messages);
  if (snip) events.push(snip);
  layer3(messages, !!opts.warm);
  const mid = estimateTokens(messages, opts.boot ?? 0);
  if (opts.force || shouldL5(mid, opts.context ?? 128000, opts.reserved ?? 8000, !!opts.warm)) {
    const b = opts.lastL5Before ?? 0;
    const a = opts.lastL5After ?? 0;
    if (b > 0 && a > b * (1 - THRASH_MIN_REDUCTION)) {
      events.push({ kind: "compact_thrash", data: { before: b, after: a } });
      return { events, before, after: mid };
    }
    const { world, ev } = layer5(messages, opts.keepTail ?? 6, opts.focus);
    const dest = path.join(home, "world", `${session}.md`);
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.writeFileSync(dest, world);
    events.push(ev);
    events.push({ kind: "compaction", data: { before_tokens: before, after_tokens: estimateTokens(messages, opts.boot ?? 0), reason: opts.force ? "operator" : "auto" } });
    return { events, world, before, after: estimateTokens(messages, opts.boot ?? 0) };
  }
  return { events, before, after: estimateTokens(messages, opts.boot ?? 0) };
}

export function callHash(name: string, input: any): string {
  return createHash("sha256").update(name).update(JSON.stringify(input)).digest("hex");
}
