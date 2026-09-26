#!/usr/bin/env node
// Validate Cosmos session events (JSONL on stdin or a file arg) against
// CONTRACT.md §6. Zero-dependency structural check of eval/event.schema.json.
import { readFileSync } from "node:fs";

const KINDS = new Set([
  "session_start", "session_end", "checkout", "user", "assistant",
  "tool_call", "tool_result", "plan", "permission_request",
  "permission_decision", "usage", "model_switch", "cache_break",
  "compaction", "subagent_start", "subagent_end", "handoff", "error", "mode",
  "snip", "compact", "compact_thrash", "tool_repair",
  "checkpoint", "verify", "self_improve", "job_start", "job_end",
  "doom_loop", "aside",
]);
const HARNESSES = new Set(["zcli", "zdev", "zagy"]);
const ULID = /^[0-9a-z]{26}$/;

const arg = process.argv[2];
const input = arg ? readFileSync(arg, "utf8") : readFileSync(0, "utf8");
const lines = input.split(/\r?\n/).filter((l) => l.trim().length > 0);

let bad = 0;
const ids = new Set();
let prevId = null;
for (const [i, line] of lines.entries()) {
  const n = i + 1;
  let ev;
  try {
    ev = JSON.parse(line);
  } catch (e) {
    console.error(`line ${n}: not JSON: ${e.message}`);
    bad++;
    continue;
  }
  const fail = (msg) => { console.error(`line ${n} [${ev.kind ?? "?"}]: ${msg}`); bad++; };
  if (ev.v !== 1) fail(`v=${ev.v}`);
  for (const k of ["id", "ts", "session", "harness", "kind", "data"])
    if (!(k in ev)) fail(`missing ${k}`);
  if (!("parent" in ev)) fail("missing parent");
  if (typeof ev.id === "string" && !ULID.test(ev.id)) fail(`id ${ev.id} not ulid`);
  if (ev.parent !== null && typeof ev.parent === "string" && !ULID.test(ev.parent))
    fail(`parent ${ev.parent} not ulid`);
  if (typeof ev.session === "string" && !ULID.test(ev.session)) fail(`session not ulid`);
  if (!HARNESSES.has(ev.harness)) fail(`harness ${ev.harness}`);
  if (!KINDS.has(ev.kind)) fail(`kind ${ev.kind}`);
  if (typeof ev.ts === "string" && Number.isNaN(Date.parse(ev.ts))) fail(`ts ${ev.ts}`);
  if (typeof ev.data !== "object" || ev.data === null) fail(`data not object`);
  for (const k of Object.keys(ev))
    if (!["v","id","parent","ts","session","harness","kind","data"].includes(k))
      fail(`extra key ${k}`);
  if (ids.has(ev.id)) fail(`dup id ${ev.id}`);
  ids.add(ev.id);
  // parent must be previous event id (chain), or a branch point that exists in file
  if (ev.parent !== null && prevId !== null && ev.parent !== prevId && !ids.has(ev.parent))
    fail(`parent ${ev.parent} not previous id ${prevId} or known id`);
  prevId = ev.id;
}
if (lines.length === 0) { console.error("no events"); bad++; }
if (bad === 0) console.log(`OK ${lines.length} events`);
process.exit(bad === 0 ? 0 : 1);
