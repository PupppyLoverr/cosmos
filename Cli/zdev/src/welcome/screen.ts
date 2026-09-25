// Full-screen terminal output for the first-run flow: a cell canvas the
// size of the terminal, drawn on the alternate screen with only changed
// rows rewritten, inside synchronized-output markers (no flicker). Ported
// from zcli-welcome's Canvas/Tty (rust-tui/crates/zwelcome).

export type Style = { fg?: string; bg?: string; bold?: boolean; italic?: boolean; underline?: boolean; inverse?: boolean };

const width = (s: string): number => (typeof Bun !== "undefined" ? Bun.stringWidth(s) : s.length);

function sgr(st: Style | undefined): string {
  if (!st) return "\x1b[0m";
  const p = ["0"];
  if (st.bold) p.push("1");
  if (st.italic) p.push("3");
  if (st.underline) p.push("4");
  if (st.inverse) p.push("7");
  const rgb = (hex: string) => {
    const n = parseInt(hex.slice(1), 16);
    return `${(n >> 16) & 255};${(n >> 8) & 255};${n & 255}`;
  };
  if (st.fg) p.push(`38;2;${rgb(st.fg)}`);
  if (st.bg) p.push(`48;2;${rgb(st.bg)}`);
  return `\x1b[${p.join(";")}m`;
}

/** A grid of styled cells; "" marks the second column of a wide char. */
export class Canvas {
  readonly w: number;
  readonly h: number;
  private ch: string[];
  private st: (Style | undefined)[];

  constructor(w: number, h: number) {
    this.w = Math.max(1, w);
    this.h = Math.max(1, h);
    this.ch = new Array(this.w * this.h).fill(" ");
    this.st = new Array(this.w * this.h).fill(undefined);
  }

  set(x: number, y: number, ch: string, st?: Style): void {
    x = Math.floor(x);
    y = Math.floor(y);
    if (x < 0 || y < 0 || x >= this.w || y >= this.h) return;
    this.ch[y * this.w + x] = ch;
    this.st[y * this.w + x] = st;
  }

  get(x: number, y: number): string {
    return x >= 0 && y >= 0 && x < this.w && y < this.h ? this.ch[y * this.w + x] : " ";
  }

  /** Write `s` at (x, y), clipped at the right edge; returns the next column. */
  text(x: number, y: number, s: string, st?: Style): number {
    let cx = Math.floor(x);
    for (const c of s) {
      if (c < " ") continue;
      const cw = width(c);
      if (cw === 0) continue;
      if (cx + cw > this.w) break;
      this.set(cx, y, c, st);
      if (cw === 2) this.set(cx + 1, y, "", st);
      cx += cw;
    }
    return cx;
  }

  spans(x: number, y: number, parts: [string, Style?][]): number {
    return parts.reduce((cx, [s, st]) => this.text(cx, y, s, st), x);
  }

  center(y: number, parts: [string, Style?][]): number {
    const w = parts.reduce((a, [s]) => a + width(s), 0);
    const x = Math.max(0, Math.floor((this.w - w) / 2));
    this.spans(x, y, parts);
    return x;
  }

  clear(x: number, y: number, w: number, h: number): void {
    for (let yy = Math.max(0, y); yy < Math.min(this.h, y + h); yy++)
      for (let xx = Math.max(0, x); xx < Math.min(this.w, x + w); xx++) this.set(xx, yy, " ");
  }

  /** One row as an ANSI string, `max` columns wide. */
  row(y: number, max = this.w): string {
    let out = "";
    let cur: Style | undefined | null = null;
    let col = 0;
    for (let x = 0; x < this.w && col < max; x++) {
      const c = this.ch[y * this.w + x];
      if (c === "") continue;
      const st = this.st[y * this.w + x];
      if (st !== cur) {
        out += sgr(st);
        cur = st;
      }
      out += c;
      col += width(c);
    }
    return out + "\x1b[0m";
  }

  plain(): string {
    const rows: string[] = [];
    for (let y = 0; y < this.h; y++) {
      let s = "";
      for (let x = 0; x < this.w; x++) s += this.ch[y * this.w + x];
      rows.push(s.replace(/\s+$/, ""));
    }
    return rows.join("\n");
  }
}

/** The alternate screen, redrawing only rows that changed. */
export class Tty {
  w: number;
  h: number;
  private prev: string[] = [];
  private out = process.stdout;

  constructor() {
    this.w = this.out.columns ?? 100;
    this.h = this.out.rows ?? 30;
  }

  enter(): void {
    process.stdin.setRawMode?.(true);
    process.stdin.resume();
    this.out.write("\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[2J");
  }

  resized(): void {
    this.w = this.out.columns ?? this.w;
    this.h = this.out.rows ?? this.h;
    this.prev = [];
  }

  draw(c: Canvas, cursor: [number, number] | null): void {
    let buf = "\x1b[?2026h\x1b[?25l";
    if (this.prev.length !== c.h) {
      buf += "\x1b[2J";
      this.prev = new Array(c.h).fill("");
    }
    for (let y = 0; y < c.h; y++) {
      // never touch the bottom-right cell: some terminals scroll on it
      const row = c.row(y, y === c.h - 1 ? c.w - 1 : c.w);
      if (row !== this.prev[y]) {
        buf += `\x1b[${y + 1};1H${row}`;
        this.prev[y] = row;
      }
    }
    if (cursor) buf += `\x1b[${cursor[1] + 1};${cursor[0] + 1}H\x1b[?25h`;
    buf += "\x1b[?2026l";
    this.out.write(buf);
  }

  /** Leave the alternate screen; `clear` wipes the terminal (scrollback too)
   * so the app starts on a clean screen. */
  leave(clear: boolean): void {
    let s = "\x1b[?2004l\x1b[?25h\x1b[0m\x1b[?1049l";
    if (clear) s += "\x1b[3J\x1b[2J\x1b[H";
    this.out.write(s);
    try {
      process.stdin.setRawMode?.(false);
    } catch {}
    process.stdin.pause();
  }
}
