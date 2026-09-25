// Raw terminal input → key events. Ink's useInput treats each stdin chunk as
// one keypress, so held arrows ("\x1b[A\x1b[A"), fast typing and pastes
// arrive garbled; this splits chunks into real keys, understands bracketed
// paste (enabled at startup), Home/End, word-jump modifiers, and tells a
// paste's newlines apart from Enter.

export type Key = {
  /** up down left right home end pageup pagedown delete insert tab return
   *  escape backspace, a ctrl letter ("a".."z", "space"), or "text"/"paste" */
  name: string;
  ctrl: boolean;
  meta: boolean;
  shift: boolean;
  /** for "text" and "paste" */
  text?: string;
};

const k = (name: string, mods: Partial<Key> = {}): Key => ({ name, ctrl: false, meta: false, shift: false, ...mods });

const CSI_NAMES: Record<string, string> = {
  A: "up", B: "down", C: "right", D: "left", H: "home", F: "end", Z: "tab",
  "1~": "home", "7~": "home", "4~": "end", "8~": "end", "3~": "delete", "2~": "insert",
  "5~": "pageup", "6~": "pagedown",
};

const PASTE_START = "\x1b[200~";
const PASTE_END = "\x1b[201~";

export class KeyParser {
  private paste: string | null = null;

  feed(chunk: string): Key[] {
    const out: Key[] = [];
    let s = chunk;
    while (s.length) {
      if (this.paste !== null) {
        const end = s.indexOf(PASTE_END);
        if (end < 0) {
          this.paste += s;
          return out;
        }
        this.paste += s.slice(0, end);
        out.push(k("paste", { text: this.paste }));
        this.paste = null;
        s = s.slice(end + PASTE_END.length);
        continue;
      }
      if (s.startsWith(PASTE_START)) {
        this.paste = "";
        s = s.slice(PASTE_START.length);
        continue;
      }
      // a terminal without bracketed paste: text with an inner line break is a paste
      const inner = s.search(/[\r\n](?=[^\r\n]*\S)/);
      if (inner > 0 && !/[\x00-\x08\x0b-\x0c\x0e-\x1f\x7f]/.test(s)) {
        out.push(k("paste", { text: s }));
        return out;
      }
      const [key, used] = this.one(s);
      if (key) out.push(key);
      s = s.slice(used);
    }
    return out;
  }

  private one(s: string): [Key | null, number] {
    const c = s[0];
    if (c === "\x1b") {
      if (s.length === 1) return [k("escape"), 1];
      if (s[1] === "[") {
        const m = s.match(/^\x1b\[([0-9;?]*)([ -/]*)([@-~])/);
        if (!m) return [k("escape"), 1];
        const params = m[1].split(";");
        const final = m[3];
        const code = final === "~" ? `${params[0]}~` : final;
        const mod = parseInt(params[1] ?? "1", 10) - 1 || 0;
        const name = CSI_NAMES[code];
        if (!name) return [null, m[0].length];
        return [
          k(name, {
            shift: !!(mod & 1) || final === "Z",
            meta: !!(mod & 2) || !!(mod & 8),
            ctrl: !!(mod & 4),
          }),
          m[0].length,
        ];
      }
      if (s[1] === "O" && s.length >= 3) {
        const name = CSI_NAMES[s[2]];
        return [name ? k(name) : null, 3];
      }
      if (s[1] === "\x7f" || s[1] === "\b") return [k("backspace", { meta: true }), 2];
      if (s[1] === "\x1b") return [k("escape", { meta: true }), 2];
      if (s[1] === "\r") return [k("return", { meta: true }), 2];
      // alt+char
      const ch = [...s.slice(1)][0];
      return [k("text", { meta: true, text: ch }), 1 + ch.length];
    }
    if (c === "\r") return [k("return"), s[1] === "\n" ? 2 : 1];
    if (c === "\n") return [k("j", { ctrl: true }), 1];
    if (c === "\t") return [k("tab"), 1];
    if (c === "\x7f" || c === "\b") return [k("backspace"), 1];
    if (c === "\x00") return [k("space", { ctrl: true }), 1];
    if (c < "\x1b") return [k(String.fromCharCode(c.charCodeAt(0) + 96), { ctrl: true }), 1];
    if (c < " ") return [null, 1];
    // a run of printable characters (surrogate pairs kept whole)
    const m = s.match(/^[^\x00-\x1f\x7f]+/)!;
    return [k("text", { text: m[0] }), m[0].length];
  }
}

// ── line editing helpers (pure) ─────────────────────────────────────────────

export type Edit = { buf: string; cur: number };

const WORD = /[\p{L}\p{N}_]/u;

export function wordLeft(buf: string, cur: number): number {
  let i = cur;
  while (i > 0 && !WORD.test(buf[i - 1])) i--;
  while (i > 0 && WORD.test(buf[i - 1])) i--;
  return i;
}

export function wordRight(buf: string, cur: number): number {
  let i = cur;
  while (i < buf.length && !WORD.test(buf[i])) i++;
  while (i < buf.length && WORD.test(buf[i])) i++;
  return i;
}

export const lineStart = (buf: string, cur: number) => buf.lastIndexOf("\n", cur - 1) + 1;
export const lineEnd = (buf: string, cur: number) => {
  const i = buf.indexOf("\n", cur);
  return i < 0 ? buf.length : i;
};

/** Move the cursor one line up/down keeping the column; null at the edge. */
export function verticalMove(buf: string, cur: number, dir: -1 | 1): number | null {
  const start = lineStart(buf, cur);
  const col = cur - start;
  if (dir < 0) {
    if (start === 0) return null;
    const prevStart = lineStart(buf, start - 1);
    return Math.min(prevStart + col, start - 1);
  }
  const end = lineEnd(buf, cur);
  if (end >= buf.length) return null;
  const nextEnd = lineEnd(buf, end + 1);
  return Math.min(end + 1 + col, nextEnd);
}
