// zdev's first-run flow — the same experience as zcli-welcome, ported from
// rust-tui/crates/zwelcome: a soft bloom of sound while you fly into a
// spiral galaxy; the field dissolves and its dust rearranges into "welcome
// to cosmos"; then your name, your keys (ChatGPT sign-in, OpenRouter,
// 9Router or any API key) with a live model list from each provider you
// connect, and a one-screen how-to. It owns the whole terminal (alternate
// screen) and clears it when done, so the app starts on a clean screen.
//
// Loaded lazily by cli.tsx only while the onboarding marker is missing.
import fs from "node:fs";
import path from "node:path";

import * as catalog from "../catalog";
import type { ModelDef, ProviderDef } from "../catalog";
import * as creds from "../creds";
import * as devin from "../devin";
import * as live from "../live";
import { PINNED, pinRank } from "../pinned";
import { cliHome } from "../proto";
import * as store from "../store";
import { KeyParser, type Key } from "../ui/keys";
import { rank } from "../ui/menus";
import { Canvas, Tty, type Style } from "./screen";
import * as sound from "./sound";

const WHITE = "#f4f4f4";
const ACCENT = "#5cd0ff";
const GREEN = "#5fd787";
const RED = "#ff6b6b";
const DIM: Style = { fg: "#8a8a8a" };
const GHOST: Style = { fg: "#6c6c6c" };
const RULE: Style = { fg: "#4a4a4a" };
const BOLD: Style = { bold: true };
const ACCENT_BOLD: Style = { fg: ACCENT, bold: true };
const SUBTITLE = "zdev v0.1.0 · the cosmos coding agent";
const QUESTION = "what should i call you?";
const STEPS = ["name", "keys", "model", "start"];
const SPIN = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const sw = (s: string) => Bun.stringWidth(s);
const ell = (s: string, w: number) => {
  if (w <= 0) return "";
  if (sw(s) <= w) return s;
  let out = "";
  for (const c of s) {
    if (sw(out + c) > w - 1) break;
    out += c;
  }
  return out + "…";
};

// ── "welcome to cosmos" in star dust ───────────────────────────────────────

/** 5×8 bitmaps: a 2-row ascender and a 6-row x-height, so at 2× every pair
 * of pixel rows fills exactly one braille cell. */
const GLYPH: Record<string, string[]> = {
  c: [".....", ".....", ".###.", "#...#", "#....", "#....", "#...#", ".###."],
  e: [".....", ".....", ".###.", "#...#", "#####", "#....", "#...#", ".###."],
  l: [".##..", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."],
  m: [".....", ".....", "##.#.", "#.#.#", "#.#.#", "#.#.#", "#.#.#", "#.#.#"],
  o: [".....", ".....", ".###.", "#...#", "#...#", "#...#", "#...#", ".###."],
  s: [".....", ".....", ".###.", "#....", ".###.", "....#", "#...#", ".###."],
  t: [".#...", ".#...", "####.", ".#...", ".#...", ".#...", ".#..#", "..##."],
  w: [".....", ".....", "#...#", "#...#", "#...#", "#.#.#", "#.#.#", ".#.#."],
};

export type Art = { rows: number; cols: number; cells: [number, number, string][] };

/** The title as braille cells; picks the biggest layout that fits. */
export function titleArt(w: number, h: number): Art {
  const layouts: [string[], number][] = [
    [["welcome to cosmos"], 3],
    [["welcome to cosmos"], 2],
    [["welcome to", "cosmos"], 2],
    [["welcome to cosmos"], 1],
  ];
  const linePx = (s: string, k: number) => Math.max(0, [...s].reduce((a, c) => a + (c === " " ? 3 * k : 6 * k), 0) - k);
  let pick = layouts[3];
  for (const l of layouts) {
    const wpx = Math.max(...l[0].map((s) => linePx(s, l[1])));
    const hpx = l[0].length * 10 * l[1] - 2 * l[1];
    if (Math.ceil(wpx / 2) + 2 <= w && Math.ceil(hpx / 4) <= h) {
      pick = l;
      break;
    }
  }
  const [lines, k] = pick;
  const wpx = Math.max(...lines.map((s) => linePx(s, k)));
  const hpx = lines.length * 10 * k - 2 * k;
  const dots = Array.from({ length: hpx }, () => new Array<boolean>(wpx).fill(false));
  lines.forEach((s, li) => {
    // centre on an even dot so every glyph stays on the braille cell grid
    let x = Math.floor((wpx - linePx(s, k)) / 2) & ~1;
    const y0 = li * 10 * k;
    for (const c of s) {
      if (c === " ") {
        x += 3 * k;
        continue;
      }
      (GLYPH[c] ?? []).forEach((row, gy) => {
        [...row].forEach((b, gx) => {
          if (b !== "#") return;
          for (let dy = 0; dy < k; dy++) for (let dx = 0; dx < k; dx++) dots[y0 + gy * k + dy][x + gx * k + dx] = true;
        });
      });
      x += 6 * k;
    }
  });
  const rows = Math.ceil(hpx / 4);
  const cols = Math.ceil(wpx / 2);
  const BITS = [
    [0x01, 0x02, 0x04, 0x40],
    [0x08, 0x10, 0x20, 0x80],
  ];
  const cells: [number, number, string][] = [];
  for (let r = 0; r < rows; r++)
    for (let c = 0; c < cols; c++) {
      let bits = 0;
      for (let dx = 0; dx < 2; dx++) for (let dy = 0; dy < 4; dy++) if (dots[r * 4 + dy]?.[c * 2 + dx]) bits |= BITS[dx][dy];
      if (bits) cells.push([c, r, String.fromCharCode(0x2800 + bits)]);
    }
  return { rows, cols, cells };
}

// ── the star field ──────────────────────────────────────────────────────────

class Rng {
  private s: bigint;
  constructor(seed: bigint) {
    this.s = seed;
  }
  next(): number {
    const M = (1n << 64n) - 1n;
    this.s ^= (this.s << 13n) & M;
    this.s ^= this.s >> 7n;
    this.s ^= (this.s << 17n) & M;
    return Number(this.s >> 40n) / 16_777_216;
  }
}

type Star = {
  x: number; y: number; z: number; v: number; l: number;
  px: number; py: number; ox: number; oy: number; tx: number; ty: number;
  delay: number; dissolve: boolean;
};

const gray = (v: number) => {
  const h = Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0");
  return `#${h}${h}${h}`;
};

function starStyle(b: number): [string, Style] {
  if (b > 0.85) return ["✶", { fg: WHITE, bold: true }];
  if (b > 0.6) return ["✦", { fg: "#d8d8d8" }];
  if (b > 0.35) return ["•", { fg: "#a8a8a8" }];
  return ["·", { fg: "#666666" }];
}

const starCount = (w: number, h: number) => Math.max(240, Math.min(640, Math.floor((w * h) / 9)));

/** A two-armed spiral seen at an angle, far off in the middle of the field. */
function galaxy(rng: Rng, n: number): Star[] {
  const out: Star[] = [];
  for (let i = 0; i < n; i++) {
    const core = i % 8 === 0;
    const r = core ? 0.07 * rng.next() ** 1.5 : 0.04 + 0.34 * rng.next() ** 0.9;
    const th = core ? rng.next() * Math.PI * 2 : (i % 2) * Math.PI + r * 9 + (rng.next() - 0.5) * 0.7;
    out.push({
      x: r * Math.cos(th), y: r * Math.sin(th) * 1.6, z: 0.92 + rng.next() * 0.08, v: 0.9 + rng.next() * 0.2,
      l: core ? 1.2 + 1.6 * (1 - r / 0.07) ** 2 : 0.7 + 0.9 * rng.next() * (1 - r),
      px: 0, py: 0, ox: 0, oy: 0, tx: 0, ty: 0, delay: 0, dissolve: false,
    });
  }
  return out;
}

type Twinkle = { x: number; y: number; phase: number; speed: number; base: number };
function sky(w: number, h: number): Twinkle[] {
  const rng = new Rng(0x5851f42d4c957f2dn ^ BigInt(w * 131 + h));
  return Array.from({ length: Math.max(12, Math.floor((w * h) / 55)) }, () => ({
    x: Math.floor(rng.next() * w), y: Math.floor(rng.next() * h), phase: rng.next() * Math.PI * 2,
    speed: 0.6 + rng.next() * 1.8, base: 0.3 + rng.next() * 0.7,
  }));
}

type Grid = ([string, Style, number] | null)[];
function plot(g: Grid, w: number, x: number, y: number, ch: string, st: Style, b: number) {
  const h = g.length / w;
  if (x < 0 || y < 0 || x >= w || y >= h) return;
  const i = Math.floor(y) * w + Math.floor(x);
  if (!g[i] || g[i]![2] < b) g[i] = [ch, st, b];
}
function blit(c: Canvas, g: Grid) {
  g.forEach((cell, i) => cell && c.set(i % c.w, Math.floor(i / c.w), cell[0], cell[1]));
}

// ── providers ───────────────────────────────────────────────────────────────

type Prov = { def: ProviderDef; name: string; pitch: string; url: string };

function providerList(): Prov[] {
  return catalog
    .providers()
    .filter((p) => catalog.isSubscription(p) || p.env.length)
    .map((p) => {
      const pin = PINNED.find((x) => x.id === p.id);
      return {
        def: p,
        name: p.kind === "chatgpt" ? "ChatGPT" : p.kind === "devin" ? "Devin" : p.name,
        pitch: pin?.pitch ?? p.env[0] ?? "",
        url: pin?.url ?? "",
      };
    })
    .sort((a, b) => pinRank(a.def.id) - pinRank(b.def.id));
}

export type Fetcher = (def: ProviderDef, signal?: AbortSignal) => Promise<ModelDef[]>;

type Phase = "warp" | "coalesce" | "name" | "keys" | "key" | "models" | "ready";

type Pick = { prov: number; loading: boolean; models: ModelDef[]; source: string; live: boolean; query: string; sel: number; ac: AbortController };

/** Make `alias` the model both CLIs open with. */
export function rememberModel(alias: string): void {
  store.setDefaultModel(alias);
  const file = path.join(cliHome(), "zcli_state.json");
  let v: Record<string, unknown> = {};
  try {
    const j = JSON.parse(fs.readFileSync(file, "utf8"));
    if (j && typeof j === "object") v = j;
  } catch {}
  v.model = alias;
  fs.writeFileSync(file, JSON.stringify(v));
}

export class Flow {
  phase: Phase = "warp";
  t0 = performance.now();
  clock = performance.now();
  w: number;
  h: number;
  stars: Star[];
  sky: Twinkle[];
  rot = 0;
  rng = new Rng(0x9e3779b97f4a7c15n);
  art: Art = { rows: 0, cols: 0, cells: [] };
  gx = 0;
  gy = 0;

  name = "";
  nc = 0;
  provs = providerList();
  filter = "";
  sel = 0;
  keyIdx = -1;
  keyBuf = "";
  unverified: number | null = null;
  oauth: { url?: string; ac: AbortController } | null = null;
  keyed = new Set<string>();
  email: string | null = null;
  pick: Pick | null = null;
  fetcher: Fetcher = live.fetchModels;
  chosen = new Map<string, string>();
  connected: string[] = [];
  model: [string, boolean] | null = null;
  status: [string, Style, number] | null = null;
  done = false;
  quit = false;

  constructor(w: number, h: number) {
    this.w = w;
    this.h = h;
    this.stars = galaxy(this.rng, starCount(w, h));
    this.sky = sky(w, h);
    this.layoutTitle();
    this.refreshKeyed();
  }

  private secs() {
    return (performance.now() - this.clock) / 1000;
  }
  private t() {
    return (performance.now() - this.t0) / 1000;
  }
  private spinner() {
    return SPIN[Math.floor(this.secs() * 12.5) % SPIN.length];
  }

  resized(w: number, h: number) {
    // Windows reports a same-size resize when the alternate screen opens —
    // it must never cut the intro short
    if (w === this.w && h === this.h) return;
    this.w = w;
    this.h = h;
    this.sky = sky(w, h);
    this.layoutTitle();
    if (this.phase === "coalesce") this.beginCoalesce();
  }

  layoutTitle() {
    this.art = titleArt(this.w - 4, this.h - 14);
    this.gx = Math.max(0, Math.floor((this.w - this.art.cols) / 2));
    this.gy = Math.max(0, Math.floor((this.h - (this.art.rows + 9)) / 2));
  }

  refreshKeyed() {
    this.keyed = new Set(
      this.provs
        .filter((p) =>
          p.def.kind === "chatgpt"
            ? !!creds.chatgpt()
            : p.def.kind === "devin"
              ? devin.knownReady()
              : !!creds.keyFor(p.def.id, p.def.env),
        )
        .map((p) => p.def.id),
    );
    this.email = creds.chatgpt()?.email ?? null;
  }

  flash(s: string, ok: boolean) {
    this.status = [s, { fg: ok ? GREEN : RED }, performance.now()];
  }

  // ---- intro ---------------------------------------------------------------

  beginCoalesce() {
    const cx = this.w / 2;
    const cy = this.h / 2;
    const n = this.art.cells.length;
    const targets = Math.max(n, Math.floor((this.stars.length * 3) / 4));
    this.stars.forEach((s, i) => {
      const on = s.px >= 0 && s.px < this.w && s.py >= 0 && s.py < this.h;
      if (on) {
        s.ox = s.px;
        s.oy = s.py;
      } else {
        const edge = Math.floor(this.rng.next() * 4);
        s.ox = edge === 0 ? 0 : edge === 1 ? this.w - 1 : this.rng.next() * this.w;
        s.oy = edge === 2 ? 0 : edge === 3 ? this.h - 1 : this.rng.next() * this.h;
      }
      s.delay = this.rng.next() * 0.5;
      if (n > 0 && i < targets) {
        const [c, r] = this.art.cells[(i * 7919) % n];
        s.tx = this.gx + c + 0.5;
        s.ty = this.gy + r + 0.5;
        s.dissolve = false;
      } else {
        s.dissolve = true;
        s.tx = s.ox + (s.ox - cx) * 0.6;
        s.ty = s.oy + (s.oy - cy) * 0.6;
      }
    });
    this.phase = "coalesce";
    this.t0 = performance.now();
  }

  drawWarp(c: Canvas, dt: number) {
    const t = this.t();
    if (t > 2.9) {
      this.beginCoalesce();
      return this.drawCoalesce(c);
    }
    const u = smooth(t / 2.6);
    const speed = 0.04 + 1.7 * u * u;
    this.rot += dt * (0.35 + 0.8 * u);
    const sn = Math.sin(this.rot);
    const cs = Math.cos(this.rot);
    const { w, h } = this;
    const cx = w / 2;
    const cy = h / 2;
    const sy = Math.min(cy, cx * 0.45);
    const g: Grid = new Array(w * h).fill(null);
    for (const s of this.stars) {
      s.z -= speed * dt * s.v;
      if (s.z < 0.07) {
        s.x = this.rng.next() * 2 - 1;
        s.y = this.rng.next() * 1.1 - 0.55;
        s.z = 0.55 + this.rng.next() * 0.45;
        s.l = 1;
      }
      const proj = (z: number): [number, number] => {
        const rx = s.x * cs - s.y * sn;
        const ry = s.x * sn + s.y * cs;
        return [cx + (rx / z) * cx * 0.92, cy + (ry / z) * sy * 0.92];
      };
      const [hx, hy] = proj(s.z);
      s.px = hx;
      s.py = hy;
      const [tx, ty] = proj(s.z + speed * dt * s.v * 3.5);
      const b = Math.min(1, (0.32 * s.l) / s.z);
      const [ch, st] = starStyle(b);
      const n = Math.max(1, Math.min(5, Math.round(Math.hypot(hx - tx, hy - ty))));
      for (let k = 0; k <= n; k++) {
        const f = k / n;
        const bk = b * (1 - f * 0.7);
        const [ck, sk] = k === 0 ? [ch, st] : starStyle(bk);
        plot(g, w, hx + (tx - hx) * f, hy + (ty - hy) * f, ck, sk, bk);
      }
    }
    blit(c, g);
    if (t > 0.6) c.text(w - sw("any key to skip") - 2, h - 2, "any key to skip", GHOST);
  }

  drawCoalesce(c: Canvas) {
    const t = this.t();
    if (t > 2.0) {
      this.beginName();
      this.drawName(c);
      return;
    }
    const { w, h } = this;
    const g: Grid = new Array(w * h).fill(null);
    for (const s of this.stars) {
      const e = smooth((t - s.delay) / 0.9);
      const x = s.ox + (s.tx - s.ox) * e;
      const y = s.oy + (s.ty - s.oy) * e;
      s.px = x;
      s.py = y;
      const [b, ch] = s.dissolve ? [0.5 * (1 - e), "·"] : [0.35 + 0.65 * e, e > 0.75 ? "✦" : "·"];
      if (b > 0.03) plot(g, w, x, y, ch, starStyle(b)[1], b);
    }
    blit(c, g);
    if (t > 1.0) {
      const reveal = Math.min(1, (t - 1.0) / 0.5);
      const span = Math.max(0, ...this.art.cells.map((a) => a[0])) + 1;
      const st: Style = { fg: WHITE, bold: true };
      for (const [x, y, ch] of this.art.cells) if (x < span * reveal) c.set(this.gx + x, this.gy + y, ch, st);
    }
    if (t > 1.3) {
      const n = Math.floor((t - 1.3) / 0.012);
      c.text(Math.floor((w - sw(SUBTITLE)) / 2), this.gy + this.art.rows + 1, [...SUBTITLE].slice(0, n).join(""), DIM);
    }
  }

  beginName() {
    sound.stop();
    this.phase = "name";
    this.t0 = performance.now();
  }

  // ---- chrome --------------------------------------------------------------

  drawSky(c: Canvas) {
    const t = this.secs();
    for (const s of this.sky) {
      const b = s.base * (0.5 + 0.5 * Math.sin(t * s.speed + s.phase));
      const [ch, v] = b > 0.85 ? ["✦", 0x70] : b > 0.45 ? ["·", 0x5a] : ["·", 0x36];
      c.set(s.x, s.y, ch, { fg: gray(v) });
    }
  }

  drawHeader(c: Canvas, step: number) {
    c.clear(0, 0, c.w, 3);
    c.text(3, 1, "✶ ", { fg: ACCENT });
    c.text(5, 1, "cosmos", BOLD);
    const parts: [string, Style?][] = [];
    STEPS.forEach((s, i) => {
      if (i) parts.push([" ─ ", RULE]);
      parts.push(i < step ? [`✓ ${s}`, { fg: GREEN }] : i === step ? [s, ACCENT_BOLD] : [s, DIM]);
    });
    const width = parts.reduce((a, [s]) => a + sw(s), 0);
    if (c.w > width + 20) c.spans(c.w - width - 3, 1, parts);
  }

  drawFooter(c: Canvas, hint: string) {
    const y = c.h - 2;
    c.clear(0, y, c.w, 1);
    c.center(y, [[hint, DIM]]);
  }

  drawStatus(c: Canvas, x: number, y: number, w: number) {
    if (this.status && performance.now() - this.status[2] < 8000) c.text(x, y, ell(this.status[0], w), this.status[1]);
  }

  column(): [number, number] {
    const cw = Math.min(100, Math.max(20, this.w - 8));
    return [Math.floor((this.w - cw) / 2), cw];
  }

  // ---- name ----------------------------------------------------------------

  drawTitle(c: Canvas) {
    const span = Math.max(1, ...this.art.cells.map((a) => a[0]));
    const pos = ((this.secs() * 0.35) % 1) * (span + 40) - 20;
    c.clear(this.gx - 2, this.gy - 1, span + 5, this.art.rows + 3);
    for (const [x, y, ch] of this.art.cells) {
      const d = (x - pos) / 6;
      c.set(this.gx + x, this.gy + y, ch, { fg: gray(200 + 55 * Math.exp(-d * d)), bold: true });
    }
    c.center(this.gy + this.art.rows + 1, [[SUBTITLE, DIM]]);
  }

  drawName(c: Canvas): [number, number] {
    this.drawSky(c);
    this.drawTitle(c);
    const y = this.gy + this.art.rows + 4;
    const bw = Math.min(34, this.w - 4);
    const x = Math.floor((this.w - bw) / 2);
    c.clear(x, y, bw, 4);
    c.center(y, [[QUESTION, BOLD]]);
    c.text(x, y + 2, "❯ ", ACCENT_BOLD);
    c.text(x + 2, y + 2, this.name);
    c.text(x, y + 3, "─".repeat(bw), RULE);
    this.drawFooter(c, "enter to continue · esc to skip");
    return [Math.min(this.w - 1, x + 2 + sw(this.name.slice(0, this.nc))), y + 2];
  }

  // ---- keys ----------------------------------------------------------------

  provOrder(): number[] {
    return rank(this.provs, this.filter, (p) => `${p.def.id} ${p.name} ${p.pitch}`);
  }

  drawKeys(c: Canvas): [number, number] | null {
    this.drawSky(c);
    this.drawHeader(c, 1);
    const [x, cw] = this.column();
    const top = 4;
    c.clear(x - 2, top, cw + 4, c.h - top - 3);
    c.text(x, top, "Bring your own keys", BOLD);
    if (this.keyed.size) {
      const s = `${this.keyed.size} connected`;
      c.text(x + cw - sw(s), top, s, { fg: GREEN });
    }
    c.text(x, top + 1, ell("Keys stay on this machine (Windows Credential Manager) and work in zcli too. Connect as many as you like.", cw), DIM);

    if (this.oauth) {
      const y = top + 4;
      c.spans(x, y, [[`${this.spinner()} `, { fg: ACCENT }], ["Signing in with ChatGPT — finish in your browser"]]);
      if (this.oauth.url) {
        c.text(x + 2, y + 2, "If it didn't open, visit:", DIM);
        c.text(x + 2, y + 3, ell(this.oauth.url, cw - 2), { fg: ACCENT });
      }
      this.drawFooter(c, "esc cancel");
      return null;
    }

    if (this.phase === "key") {
      const p = this.provs[this.keyIdx];
      const y = top + 4;
      c.text(x, y, `${p.name} API key`, BOLD);
      c.text(x, y + 1, "Paste or type it — it's hidden as you type, and checked with the provider.", DIM);
      c.text(x, y + 3, "❯ ", ACCENT_BOLD);
      const dots = "•".repeat(Math.min(this.keyBuf.length, cw - 4));
      c.text(x + 2, y + 3, dots);
      c.text(x, y + 4, "─".repeat(Math.min(cw, 64)), RULE);
      let yy = y + 6;
      if (p.url) {
        c.spans(x, yy++, [["get one at ", DIM], [p.url, { fg: ACCENT }]]);
      }
      if (p.def.env[0]) c.text(x, yy++, `or set ${p.def.env[0]} in your environment`, DIM);
      this.drawStatus(c, x, yy + 1, cw);
      this.drawFooter(c, "enter save · esc back");
      return [x + 2 + dots.length, y + 3];
    }

    const fy = top + 3;
    c.text(x, fy, "/ ", DIM);
    if (this.filter) c.text(x + 2, fy, this.filter);
    else c.text(x + 2, fy, `Type to search ${this.provs.length} providers`, GHOST);
    c.text(x, fy + 1, "─".repeat(cw), RULE);
    const order = this.provOrder();
    const items = order.length + 1;
    this.sel = Math.min(this.sel, items - 1);
    const listY = fy + 2;
    const rows = Math.max(3, c.h - listY - 6);
    const first = Math.max(0, this.sel - rows + 1);
    const nameW = Math.min(30, Math.max(...this.provs.map((p) => sw(p.name)))) + 2;
    for (let r = 0, vi = first; vi < Math.min(items, first + rows); vi++, r++) {
      const y = listY + r;
      const on = vi === this.sel;
      c.text(x, y, on ? "❯ " : "  ", ACCENT_BOLD);
      if (vi === order.length) {
        c.text(x + 2, y, this.model || this.keyed.size ? "Continue →" : "Skip for now →", on ? ACCENT_BOLD : DIM);
        continue;
      }
      const p = this.provs[order[vi]];
      c.text(x + 2, y, ell(p.name, nameW - 2), on ? ACCENT_BOLD : undefined);
      const m = this.chosen.get(p.def.id);
      const right = m
        ? `✓ ${m}${this.model?.[0] === `${p.def.id}/${m}` ? " ★" : ""}`
        : this.keyed.has(p.def.id)
          ? p.def.kind === "chatgpt" && this.email
            ? `✓ ${this.email}`
            : p.def.kind === "devin"
              ? `✓ ${devin.detail()}`
              : "✓ connected"
          : "";
      const rw = Math.min(sw(right), Math.floor(cw / 2));
      c.text(x + 2 + nameW, y, ell(p.pitch, cw - 2 - nameW - rw - 2), DIM);
      if (rw) c.text(x + cw - rw, y, ell(right, rw), { fg: GREEN });
    }
    const by = listY + rows;
    c.text(x, by, "─".repeat(cw), RULE);
    const below = items - first - rows;
    if (below > 0) c.text(x + 2, by, ` ↓ ${below} more `, DIM);
    this.drawStatus(c, x, by + 1, cw);
    this.drawFooter(c, "↑↓ choose · enter connect or pick a model · type to search · esc continue");
    return [x + 2 + sw(this.filter), fy];
  }

  // ---- models --------------------------------------------------------------

  openModels(i: number) {
    const ac = new AbortController();
    const def = this.provs[i].def;
    this.pick = { prov: i, loading: true, models: [], source: "", live: false, query: "", sel: 0, ac };
    this.phase = "models";
    this.fetcher(def, ac.signal).then(
      (models) => this.landModels(i, ac, models, null),
      (e) => this.landModels(i, ac, null, e),
    );
  }

  landModels(i: number, ac: AbortController, models: ModelDef[] | null, err: any) {
    if (!this.pick || this.pick.ac !== ac || ac.signal.aborted) return;
    const def = this.provs[i].def;
    if (models) {
      live.store(def.id, models);
      Object.assign(this.pick, { loading: false, models, live: true, source: `${models.length} models live from ${catalog.hostOf(def.base_url)}` });
      if (this.unverified === i) this.unverified = null;
      return;
    }
    const msg = String(err?.message ?? err);
    if (err instanceof live.RejectedKey && this.unverified === i) {
      creds.deleteKey(def.id);
      this.unverified = null;
      this.refreshKeyed();
      this.pick = null;
      this.keyBuf = "";
      this.keyIdx = i;
      this.phase = "key";
      this.flash(`✗ ${msg} — check it and try again`, false);
      return;
    }
    Object.assign(this.pick, {
      loading: false,
      models: def.models,
      live: false,
      source: `couldn't fetch the live list (${msg}) — showing known models`,
    });
    this.unverified = null;
  }

  modelOrder(): number[] {
    if (!this.pick) return [];
    return rank(this.pick.models, this.pick.query, (m) => `${m.id} ${m.name}`);
  }

  drawModels(c: Canvas): [number, number] | null {
    this.drawSky(c);
    this.drawHeader(c, 2);
    const [x, cw] = this.column();
    const top = 4;
    c.clear(x - 2, top, cw + 4, c.h - top - 3);
    const pk = this.pick!;
    const p = this.provs[pk.prov];
    c.text(x, top, `Choose a model — ${p.name}`, BOLD);
    if (pk.loading) {
      c.spans(x, top + 1, [[`${this.spinner()} `, { fg: ACCENT }], [`fetching live models from ${catalog.hostOf(p.def.base_url)}…`, DIM]]);
      this.drawStatus(c, x, top + 3, cw);
      this.drawFooter(c, "esc back");
      return null;
    }
    c.text(x, top + 1, ell(`${pk.source} · the one you pick becomes your default`, cw), pk.live ? DIM : { fg: RED });
    const fy = top + 3;
    c.text(x, fy, "/ ", DIM);
    if (pk.query) c.text(x + 2, fy, pk.query);
    else c.text(x + 2, fy, `Type to search ${pk.models.length} models`, GHOST);
    const ctxW = 7;
    const priceW = 17;
    const idW = Math.max(10, cw - 2 - ctxW - priceW - 4);
    const cxCtx = x + 2 + idW + 2;
    const cxPrice = x + 2 + idW + 4 + ctxW;
    c.text(x + 2, fy + 1, "model", DIM);
    c.text(cxCtx, fy + 1, "context".padStart(ctxW), DIM);
    c.text(cxPrice, fy + 1, "$/M in / out".padStart(priceW), DIM);
    c.text(x, fy + 2, "─".repeat(cw), RULE);
    const order = this.modelOrder();
    const listY = fy + 3;
    const rows = Math.max(3, c.h - listY - 6);
    const sel = Math.min(pk.sel, Math.max(0, order.length - 1));
    const first = Math.max(0, sel - rows + 1);
    for (let r = 0, vi = first; vi < Math.min(order.length, first + rows); vi++, r++) {
      const y = listY + r;
      const on = vi === sel;
      const m = pk.models[order[vi]];
      c.text(x, y, on ? "❯ " : "  ", ACCENT_BOLD);
      const star = this.model?.[0] === `${p.def.id}/${m.id}` ? " ★" : "";
      c.text(x + 2, y, ell(m.id + star, idW), on ? ACCENT_BOLD : undefined);
      c.text(cxCtx, y, kfmt(m.context).padStart(ctxW), DIM);
      const price = catalog.isSubscription(p.def)
        ? "included"
        : m.price[0] || m.price[1]
          ? `${money(m.price[0])} / ${money(m.price[1])}`
          : p.def.id === "openrouter" || m.id.endsWith(":free")
            ? "free"
            : "";
      c.text(cxPrice, y, ell(price, priceW).padStart(priceW), DIM);
    }
    if (!order.length) c.text(x + 2, listY, "no model matches", DIM);
    const by = listY + rows;
    c.text(x, by, "─".repeat(cw), RULE);
    const below = order.length - first - rows;
    if (below > 0) c.text(x + 2, by, ` ↓ ${below} more `, DIM);
    const cur = order[sel] !== undefined ? pk.models[order[sel]] : null;
    if (cur && cur.name !== cur.id) c.text(x, by + 1, ell(cur.name, cw), DIM);
    this.drawFooter(c, "↑↓ choose · enter use this model · type to search · esc back");
    return [x + 2 + sw(pk.query), fy];
  }

  chooseModel() {
    const order = this.modelOrder();
    const pk = this.pick;
    if (!pk || !order.length) return;
    const m = pk.models[order[Math.min(pk.sel, order.length - 1)]];
    const def = this.provs[pk.prov].def;
    const alias = `${def.id}/${m.id}`;
    try {
      rememberModel(alias);
      this.chosen.set(def.id, m.id);
      this.model = [alias, true];
      this.flash(`✓ default model → ${alias}`, true);
    } catch (e: any) {
      this.flash(`✗ couldn't write config.toml: ${e?.message ?? e}`, false);
    }
    const prov = pk.prov;
    this.pick = null;
    this.phase = "keys";
    this.filter = "";
    this.sel = Math.max(0, this.provOrder().indexOf(prov));
  }

  // ---- ready ---------------------------------------------------------------

  afterKeys() {
    if (!this.model) {
      const cfg = store.loadConfig(cliHome());
      const d = cfg.default_model;
      if (d && d !== "fixture") this.model = [d, false];
      else {
        const models = store.availableModels(cfg);
        const pick =
          this.connected.map((pid) => models.find((m) => !m.fromConfig && m.provider === pid)).find(Boolean) ??
          models.find((m) => !m.fromConfig && this.keyed.has(m.provider));
        if (pick) {
          try {
            rememberModel(pick.alias);
            this.model = [pick.alias, true];
          } catch (e: any) {
            this.flash(`✗ couldn't write config.toml: ${e?.message ?? e}`, false);
          }
        }
      }
    }
    this.phase = "ready";
  }

  drawReady(c: Canvas) {
    const ROWS: [string, string][] = [
      ["type a task, enter", "zdev reads, edits and runs code with you"],
      ["/", "commands — /model /providers /connect /usage /context /help"],
      ["@path", "attach a file · !cmd runs a shell command"],
      ["ctrl+space", "dictate instead of typing"],
      ["shift+tab", "cycle agent → plan → yolo"],
      ["esc", "interrupt the agent · ctrl+c twice quits"],
      ['zdev -p "…"', "headless runs for scripts and CI"],
    ];
    this.drawSky(c);
    this.drawHeader(c, 3);
    const name = this.name.trim();
    const kw = Math.max(...ROWS.map(([k]) => sw(k)));
    const dw = Math.max(...ROWS.map(([, d]) => sw(d)));
    const bw = Math.min(Math.max(kw + 3 + dw, 60), this.w - 4);
    const bh = ROWS.length + 7;
    const x = Math.floor((this.w - bw) / 2);
    const y = Math.max(0, Math.floor((this.h - bh) / 2));
    c.clear(x - 2, y - 1, bw + 4, bh + 2);
    c.spans(x, y, [["✶ ", { fg: ACCENT }], [name ? `You're all set, ${name}.` : "You're all set.", BOLD]]);
    if (this.model) c.spans(x, y + 2, [["model  ", DIM], [this.model[0], BOLD], ["  · /model to switch", DIM]]);
    else c.text(x, y + 2, "no provider yet — /connect or /login inside zdev when you're ready", DIM);
    const keys = this.provs.filter((p) => this.keyed.has(p.def.id)).map((p) => p.name);
    if (keys.length) c.spans(x, y + 3, [["keys   ", DIM], [ell(keys.join(" · "), bw - 7), { fg: GREEN }]]);
    ROWS.forEach(([k, d], i) => {
      c.text(x, y + 5 + i, k.padEnd(kw), { fg: ACCENT });
      c.text(x + kw + 3, y + 5 + i, d, DIM);
    });
    c.spans(x, y + 6 + ROWS.length, [["enter ", ACCENT_BOLD], ["launch zdev"]]);
    this.drawFooter(c, "enter launch · esc back to keys");
  }

  // ---- frame + input -------------------------------------------------------

  frame(dt: number): [Canvas, [number, number] | null] {
    const c = new Canvas(this.w, this.h);
    let cursor: [number, number] | null = null;
    switch (this.phase) {
      case "warp":
        this.drawWarp(c, dt);
        break;
      case "coalesce":
        this.drawCoalesce(c);
        break;
      case "name":
        cursor = this.drawName(c);
        break;
      case "keys":
      case "key":
        cursor = this.drawKeys(c);
        break;
      case "models":
        cursor = this.drawModels(c);
        break;
      case "ready":
        this.drawReady(c);
        break;
    }
    return [c, cursor];
  }

  submitKey() {
    const key = this.keyBuf.trim();
    this.keyBuf = "";
    if (!key) {
      this.phase = "keys";
      return;
    }
    const p = this.provs[this.keyIdx];
    try {
      const where = creds.storeKey(p.def.id, key);
      this.flash(`✓ ${p.name} key saved to the ${where} — checking it…`, true);
      this.unverified = this.keyIdx;
      this.connected.push(p.def.id);
      this.refreshKeyed();
      this.openModels(this.keyIdx);
    } catch (e: any) {
      this.flash(`✗ ${p.name}: ${e?.message ?? e}`, false);
      this.phase = "keys";
    }
  }

  connectDevin(i: number) {
    devin.invalidate();
    if (devin.ready()) {
      this.connected.push("devin");
      this.refreshKeyed();
      this.flash(`✓ Devin · ${devin.detail()}`, true);
      this.openModels(i);
    } else {
      this.flash(devin.hint(), false);
    }
  }

  startOauth() {
    const ac = new AbortController();
    this.oauth = { ac };
    import("../oauth")
      .then((o) => o.login((url) => this.oauth && (this.oauth.url = url), ac.signal))
      .then(
        (t) => {
          if (ac.signal.aborted) return;
          this.oauth = null;
          this.refreshKeyed();
          this.connected.push("chatgpt");
          this.flash(`✓ signed in${t.email ? ` as ${t.email}` : ""}`, true);
          const i = this.provs.findIndex((p) => p.def.kind === "chatgpt");
          if (i >= 0) this.openModels(i);
        },
        (e) => {
          if (ac.signal.aborted) return;
          this.oauth = null;
          this.flash(`✗ sign-in failed: ${e?.message ?? e}`, false);
        },
      );
  }

  finish() {
    const name = this.name.trim();
    if (name) store.setUserName(name);
    this.done = true;
  }

  onKey(k: Key) {
    if (k.ctrl && (k.name === "c" || k.name === "d")) {
      this.oauth?.ac.abort();
      this.pick?.ac.abort();
      sound.stop();
      this.quit = true;
      this.done = true;
      return;
    }
    const edit = (s: string, cur: number, max: number): [string, number] => {
      if (k.name === "backspace") return cur > 0 ? [s.slice(0, cur - 1) + s.slice(cur), cur - 1] : [s, cur];
      if (k.name === "delete") return [s.slice(0, cur) + s.slice(cur + 1), cur];
      if (k.name === "left") return [s, Math.max(0, cur - 1)];
      if (k.name === "right") return [s, Math.min(s.length, cur + 1)];
      if (k.name === "home") return [s, 0];
      if (k.name === "end") return [s, s.length];
      const add = k.name === "paste" ? (k.text ?? "").replace(/[\r\n]+/g, " ").trim() : k.name === "text" && !k.ctrl ? (k.text ?? "") : "";
      if (add) {
        const room = Math.max(0, max - s.length);
        const a = add.slice(0, room);
        return [s.slice(0, cur) + a + s.slice(cur), cur + a.length];
      }
      return [s, cur];
    };
    switch (this.phase) {
      case "warp":
        sound.stop();
        return this.beginCoalesce();
      case "coalesce":
        return this.beginName();
      case "name":
        if (k.name === "return") return void (this.phase = "keys");
        if (k.name === "escape") {
          this.name = "";
          this.nc = 0;
          this.phase = "keys";
          return;
        }
        [this.name, this.nc] = edit(this.name, this.nc, 40);
        return;
      case "keys": {
        if (this.oauth) {
          if (k.name === "escape") {
            this.oauth.ac.abort();
            this.oauth = null;
            this.flash("sign-in cancelled", false);
          }
          return;
        }
        const order = this.provOrder();
        const items = order.length + 1;
        if (k.name === "escape") return this.afterKeys();
        if (k.name === "up" || (k.name === "tab" && k.shift)) this.sel = this.sel === 0 ? items - 1 : Math.min(this.sel, items) - 1;
        else if (k.name === "down" || k.name === "tab") this.sel = this.sel + 1 >= items ? 0 : this.sel + 1;
        else if (k.name === "pagedown") this.sel = Math.min(items - 1, this.sel + 10);
        else if (k.name === "pageup") this.sel = Math.max(0, this.sel - 10);
        else if (k.name === "home") this.sel = 0;
        else if (k.name === "end") this.sel = items - 1;
        else if (k.name === "backspace") {
          this.filter = this.filter.slice(0, -1);
          this.sel = 0;
        } else if (k.name === "return") {
          const sel = Math.min(this.sel, items - 1);
          if (sel === order.length) return this.afterKeys();
          const i = order[sel];
          this.status = null;
          if (this.keyed.has(this.provs[i].def.id)) this.openModels(i);
          else if (this.provs[i].def.kind === "chatgpt") this.startOauth();
          else if (this.provs[i].def.kind === "devin") this.connectDevin(i);
          else {
            this.keyBuf = "";
            this.keyIdx = i;
            this.phase = "key";
          }
        } else if ((k.name === "text" || k.name === "paste") && !k.ctrl) {
          this.filter += (k.text ?? "").replace(/[\r\n]/g, "");
          this.sel = 0;
        }
        return;
      }
      case "key":
        if (k.name === "escape") {
          this.keyBuf = "";
          this.phase = "keys";
          return;
        }
        if (k.name === "return") return this.submitKey();
        [this.keyBuf] = edit(this.keyBuf, this.keyBuf.length, 400);
        return;
      case "models": {
        const pk = this.pick;
        if (k.name === "escape") {
          pk?.ac.abort();
          this.unverified = null;
          this.pick = null;
          this.phase = "keys";
          return;
        }
        if (!pk || pk.loading) return;
        const n = this.modelOrder().length;
        if (k.name === "return") return this.chooseModel();
        if (k.name === "up" || (k.name === "tab" && k.shift)) pk.sel = pk.sel === 0 ? Math.max(0, n - 1) : pk.sel - 1;
        else if (k.name === "down" || k.name === "tab") pk.sel = pk.sel + 1 >= n ? 0 : pk.sel + 1;
        else if (k.name === "pagedown") pk.sel = Math.min(Math.max(0, n - 1), pk.sel + 10);
        else if (k.name === "pageup") pk.sel = Math.max(0, pk.sel - 10);
        else if (k.name === "home") pk.sel = 0;
        else if (k.name === "end") pk.sel = Math.max(0, n - 1);
        else if (k.name === "backspace") {
          pk.query = pk.query.slice(0, -1);
          pk.sel = 0;
        } else if ((k.name === "text" || k.name === "paste") && !k.ctrl) {
          pk.query += (k.text ?? "").replace(/[\r\n]/g, "");
          pk.sel = 0;
        }
        return;
      }
      case "ready":
        if (k.name === "return") return this.finish();
        if (k.name === "escape") this.phase = "keys";
        return;
    }
  }
}

function smooth(x: number) {
  x = Math.min(1, Math.max(0, x));
  return x * x * (3 - 2 * x);
}

function kfmt(n: number): string {
  if (n >= 1_000_000) {
    const m = n / 1_048_576;
    return Math.abs(m - Math.round(m)) < 0.05 ? `${Math.round(m)}M` : `${m.toFixed(1)}M`;
  }
  return n >= 1000 ? `${Math.round(n / 1000)}k` : String(n);
}

function money(x: number): string {
  return `$${x.toFixed(2).replace(/\.?0+$/, "")}`;
}

/** Run the flow on the real terminal. Resolves true when completed, false
 * on Ctrl+C. The terminal is cleared on completion. */
export async function runWelcome(): Promise<boolean> {
  const tty = new Tty();
  tty.enter();
  const flow = new Flow(tty.w, tty.h);
  const parser = new KeyParser();
  const onData = (d: Buffer | string) => {
    for (const k of parser.feed(String(d))) flow.onKey(k);
  };
  const onResize = () => {
    const [w, h] = [process.stdout.columns ?? tty.w, process.stdout.rows ?? tty.h];
    if (w === tty.w && h === tty.h) return;
    tty.resized();
    flow.resized(tty.w, tty.h);
  };
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", onData);
  process.stdout.on("resize", onResize);
  sound.play();
  let last = performance.now();
  try {
    while (!flow.done) {
      const now = performance.now();
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      const [canvas, cursor] = flow.frame(dt);
      tty.draw(canvas, cursor);
      await new Promise((r) => setTimeout(r, 16));
    }
  } finally {
    process.stdin.off("data", onData);
    process.stdout.off("resize", onResize);
    sound.stop();
    tty.leave(!flow.quit);
  }
  return !flow.quit;
}
