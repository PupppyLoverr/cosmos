// zproto — wire types shared by every Cosmos CLI harness (CONTRACT.md §6).
import os from "node:os";
import path from "node:path";
import { randomBytes } from "node:crypto";

export const CONTRACT_VERSION = 1;

// exit codes (contract §7)
export const EXIT_DONE = 0;
export const EXIT_FAILED = 1;
export const EXIT_USAGE = 2;
export const EXIT_DENIED = 3;
export const EXIT_PROVIDER = 4;
export const EXIT_BUDGET = 5;

const CROCKFORD = "0123456789abcdefghjkmnpqrstvwxyz";

export function newId(): string {
  // ULID: 48-bit ms timestamp + 80-bit randomness, Crockford base32.
  const t = BigInt(Date.now()) & ((1n << 48n) - 1n);
  const r = randomBytes(10);
  let bits = (t << 80n) | BigInt("0x" + r.toString("hex"));
  const chars: string[] = [];
  for (let i = 0; i < 26; i++) {
    chars.push(CROCKFORD[Number(bits & 31n)]);
    bits >>= 5n;
  }
  return chars.reverse().join("");
}

export function nowTs(): string {
  // RFC3339 with millisecond precision — same shape as the Python harness.
  return new Date().toISOString();
}

export type Event = {
  v: number;
  id: string;
  parent: string | null;
  ts: string;
  session: string;
  harness: string;
  kind: string;
  data: Record<string, any>;
};

export function event(
  session: string,
  harness: string,
  parent: string | null,
  kind: string,
  data: Record<string, any>,
): Event {
  return {
    v: CONTRACT_VERSION,
    id: newId(),
    parent,
    ts: nowTs(),
    session,
    harness,
    kind,
    data,
  };
}

export function toLine(ev: Event): string {
  return JSON.stringify(ev);
}

export class RunOpts {
  prompt: string | null = null;
  output = "text"; // text|json|json-stream
  mode: string | null = null; // plan|agent
  allow: string[] = [];
  yolo = false;
  no_sandbox = false;
  model: string | null = null;
  max_turns: number | null = null;
  max_usd: number | null = null;
  cwd: string | null = null;
  resume: string | null = null;
  add_dirs: string[] = [];
  system_prompt: string | null = null;
  append_system_prompt: string | null = null;
  approve_plan = false;
  allow_escalation: string[] = [];
  sandbox: string | null = null;
  ask: string | null = null;
  autonomous = false;
  autonomous_gate: string | null = null;
  allow_self_mutate = false;
  arena = false;
  self_improve: boolean | null = null;
}

export function cliHome(): string {
  const home = process.env.COSMOS_CLI_HOME;
  if (home) return home;
  const base = process.env.HOME || process.env.USERPROFILE || os.homedir();
  return path.join(base, ".cosmos-cli");
}
