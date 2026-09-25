// Minimal TOML parser — covers the config.toml surface: [tables], [a.b.c],
// [[array-of-tables]], basic/literal strings, ints, floats, bools, arrays,
// inline tables, dotted keys, # comments. Not a full TOML impl (no multiline
// strings, no dates) — enough for CONTRACT.md §1 config.

type Obj = Record<string, any>;

function stripComment(line: string): string {
  let inStr = false;
  let strCh = "";
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (inStr) {
      if (c === "\\" && strCh === '"') i++;
      else if (c === strCh) inStr = false;
      continue;
    }
    if (c === '"' || c === "'") {
      inStr = true;
      strCh = c;
    } else if (c === "#") {
      return line.slice(0, i);
    }
  }
  return line;
}

function splitTop(s: string, sep: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let inStr = false;
  let strCh = "";
  let cur = "";
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (inStr) {
      cur += c;
      if (c === "\\" && strCh === '"') {
        cur += s[i + 1] ?? "";
        i++;
      } else if (c === strCh) inStr = false;
      continue;
    }
    if (c === '"' || c === "'") {
      inStr = true;
      strCh = c;
      cur += c;
    } else if (c === "[" || c === "{") {
      depth++;
      cur += c;
    } else if (c === "]" || c === "}") {
      depth--;
      cur += c;
    } else if (c === sep && depth === 0) {
      out.push(cur);
      cur = "";
    } else {
      cur += c;
    }
  }
  if (cur.trim() || out.length) out.push(cur);
  return out;
}

function parseKey(k: string): string[] {
  return splitTop(k.trim(), ".").map((p) => {
    p = p.trim();
    if (
      (p.startsWith('"') && p.endsWith('"')) ||
      (p.startsWith("'") && p.endsWith("'"))
    ) {
      return parseString(p);
    }
    return p;
  });
}

function parseString(s: string): string {
  const q = s[0];
  const body = s.slice(1, -1);
  if (q === "'") return body;
  // basic string: process escapes
  return body.replace(/\\(u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8}|.)/g, (_, e) => {
    switch (e[0]) {
      case "n": return "\n";
      case "t": return "\t";
      case "r": return "\r";
      case '"': return '"';
      case "\\": return "\\";
      case "b": return "\b";
      case "f": return "\f";
      case "u": return String.fromCodePoint(parseInt(e.slice(1), 16));
      case "U": return String.fromCodePoint(parseInt(e.slice(1), 16));
      default: return e;
    }
  });
}

function parseValue(s: string): any {
  s = s.trim();
  if (s.startsWith('"') && s.endsWith('"') && s.length >= 2)
    return parseString(s);
  if (s.startsWith("'") && s.endsWith("'") && s.length >= 2)
    return s.slice(1, -1);
  if (s === "true") return true;
  if (s === "false") return false;
  if (s.startsWith("[") && s.endsWith("]")) {
    const inner = s.slice(1, -1).trim();
    if (!inner) return [];
    return splitTop(inner, ",").map(parseValue);
  }
  if (s.startsWith("{") && s.endsWith("}")) {
    const inner = s.slice(1, -1).trim();
    const obj: Obj = {};
    if (!inner) return obj;
    for (const pair of splitTop(inner, ",")) {
      const eq = pair.indexOf("=");
      if (eq < 0) continue;
      const keys = parseKey(pair.slice(0, eq));
      setPath(obj, keys, parseValue(pair.slice(eq + 1)));
    }
    return obj;
  }
  const num = s.replace(/_/g, "");
  if (/^[+-]?\d+$/.test(num)) return parseInt(num, 10);
  if (/^[+-]?(\d+\.\d*|\.\d+|\d+)([eE][+-]?\d+)?$/.test(num))
    return parseFloat(num);
  return s; // bare token — keep as string
}

function setPath(obj: Obj, keys: string[], val: any) {
  let cur = obj;
  for (let i = 0; i < keys.length - 1; i++) {
    const k = keys[i];
    if (typeof cur[k] !== "object" || cur[k] === null || Array.isArray(cur[k]))
      cur[k] = {};
    cur = cur[k];
  }
  cur[keys[keys.length - 1]] = val;
}

function getPath(obj: Obj, keys: string[]): Obj {
  let cur = obj;
  for (const k of keys) {
    if (typeof cur[k] !== "object" || cur[k] === null || Array.isArray(cur[k]))
      cur[k] = {};
    cur = cur[k];
  }
  return cur;
}

export function parse(text: string): Obj {
  const root: Obj = {};
  let current: Obj = root;
  for (const raw of text.split(/\r?\n/)) {
    const line = stripComment(raw).trim();
    if (!line) continue;
    if (line.startsWith("[[") && line.endsWith("]]")) {
      const keys = parseKey(line.slice(2, -2));
      const parentKeys = keys.slice(0, -1);
      const leaf = keys[keys.length - 1];
      const parent = getPath(root, parentKeys);
      if (!Array.isArray(parent[leaf])) parent[leaf] = [];
      const item: Obj = {};
      (parent[leaf] as Obj[]).push(item);
      current = item;
      continue;
    }
    if (line.startsWith("[") && line.endsWith("]")) {
      current = getPath(root, parseKey(line.slice(1, -1)));
      continue;
    }
    const eq = line.indexOf("=");
    if (eq < 0) continue;
    setPath(current, parseKey(line.slice(0, eq)), parseValue(line.slice(eq + 1)));
  }
  return root;
}
