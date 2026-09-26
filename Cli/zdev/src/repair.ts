// Tool-call repair. Same algorithm as ztools/src/repair.rs — keep in lockstep.
// 1 unwrap fences  2 alias remap  3 optional defaults + required  4 coerce  5 drop unknown keys

import { TOOLS_JSON } from "./tools";

export type Repair = { from: string; to: string; reason: string };
export type RepairResult = {
  name: string;
  input: Record<string, any>;
  repairs: Repair[];
  reject?: string;
};

const ALIAS: [string, string][] = [
  ["Read", "read"],
  ["read_file", "read"],
  ["file_read", "read"],
  ["Write", "write"],
  ["write_file", "write"],
  ["file_write", "write"],
  ["Edit", "edit"],
  ["StrReplace", "edit"],
  ["str_replace", "edit"],
  ["Bash", "bash"],
  ["Shell", "bash"],
  ["shell", "bash"],
  ["Grep", "grep"],
  ["Glob", "glob"],
  ["Ls", "ls"],
  ["List", "ls"],
  ["MemorySearch", "memory_search"],
  ["memory.search", "memory_search"],
  ["MemoryWrite", "memory_write"],
  ["Task", "task"],
  ["TodoWrite", "task"],
];

function specs(): any[] {
  return JSON.parse(TOOLS_JSON);
}

export function unwrapJson(raw: string): string {
  let s = raw.trim();
  if (s.startsWith("```json")) s = s.slice(7);
  else if (s.startsWith("```")) s = s.slice(3);
  if (s.endsWith("```")) s = s.slice(0, -3);
  s = s.trim();
  const a = s.indexOf("{");
  const b = s.lastIndexOf("}");
  if (a >= 0 && b > a) return s.slice(a, b + 1);
  if (a >= 0) {
    let body = s.slice(a);
    const opens = (body.match(/{/g) ?? []).length;
    const closes = (body.match(/}/g) ?? []).length;
    for (let i = 0; i < opens - closes; i++) body += "}";
    return body;
  }
  return s;
}

function remapName(name: string): { name: string; repair?: Repair } {
  const trimmed = name.trim();
  for (const [from, to] of ALIAS) {
    if (trimmed === from || (trimmed.toLowerCase() === from.toLowerCase() && from !== to)) {
      if (trimmed !== to) return { name: to, repair: { from: trimmed, to, reason: "alias" } };
    }
  }
  for (const s of specs()) {
    const n = s.name as string;
    if (trimmed.toLowerCase() === n.toLowerCase() && trimmed !== n) {
      return { name: n, repair: { from: trimmed, to: n, reason: "case" } };
    }
  }
  return { name: trimmed };
}

function coerce(v: any, ty: string): any {
  if (ty === "integer") {
    if (typeof v === "number" && Number.isFinite(v)) return Math.trunc(v);
    if (typeof v === "string" && /^-?\d+$/.test(v.trim())) return parseInt(v.trim(), 10);
    return undefined;
  }
  if (ty === "boolean") {
    if (typeof v === "boolean") return v;
    if (typeof v === "string") return v === "true" || v === "1" || v === "yes";
    return undefined;
  }
  if (ty === "string") {
    return typeof v === "string" ? v : String(v);
  }
  if (ty === "object") return v && typeof v === "object" && !Array.isArray(v) ? v : undefined;
  if (ty === "array") return Array.isArray(v) ? v : undefined;
  return v;
}

export function repair(name: string, input: any): RepairResult {
  const repairs: Repair[] = [];
  const remapped = remapName(name);
  let n = remapped.name;
  if (remapped.repair) repairs.push(remapped.repair);

  let obj: Record<string, any> = {};
  if (input && typeof input === "object" && !Array.isArray(input)) obj = { ...input };
  else if (typeof input === "string") {
    try {
      const parsed = JSON.parse(unwrapJson(input));
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        obj = parsed;
        repairs.push({ from: "string", to: "object", reason: "unwrap" });
      }
    } catch {
      obj = {};
    }
  }

  const spec = specs().find((s) => s.name === n);
  if (spec) {
    const props = spec.input_schema?.properties ?? {};
    const known = Object.keys(props);
    for (const k of Object.keys(obj)) {
      if (!known.includes(k)) {
        delete obj[k];
        repairs.push({ from: k, to: "", reason: "unknown_key" });
      }
    }
    for (const k of known) {
      const ty = props[k]?.type;
      if (ty && k in obj) {
        const c = coerce(obj[k], ty);
        if (c !== undefined && c !== obj[k]) {
          repairs.push({ from: `${k}:${obj[k]}`, to: `${k}:${c}`, reason: "coerce" });
          obj[k] = c;
        }
      }
    }
    for (const req of spec.input_schema?.required ?? []) {
      const v = obj[req];
      if (v == null || v === "") {
        return { name: n, input: obj, repairs, reject: `repair: missing field ${req} — resend` };
      }
    }
  }
  return { name: n, input: obj, repairs };
}
