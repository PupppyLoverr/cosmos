// Galaxy — the Cosmos mark. Not a fixed ASCII art string: a density field
// splatted from a real two-armed logarithmic spiral + gaussian core, then
// thresholded into dot glyphs. Deterministic seed → identical every render.
import React from "react";
import { Text } from "ink";

function mulberry(seed: number) {
  return () => {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Splat the spiral into a density field. */
export function galaxy(w: number, h: number, seed = 7): number[][] {
  const rnd = mulberry(seed);
  const d: number[][] = Array.from({ length: h }, () => new Array(w).fill(0));
  const cx = w / 2;
  const cy = h / 2;
  const tilt = 0.52; // face-on tilt
  const R = Math.min(w * 0.44, (h / tilt) * 0.52);
  const splat = (x: number, y: number, wgt: number) => {
    const ix = Math.round(x);
    const iy = Math.round(y);
    if (ix >= 0 && ix < w && iy >= 0 && iy < h) d[iy][ix] += wgt;
  };
  for (let i = 0; i < 160; i++) {
    const r = Math.abs(rnd() + rnd() - 1) * 2.3;
    const a = rnd() * Math.PI * 2;
    splat(cx + Math.cos(a) * r * 1.9, cy + Math.sin(a) * r * tilt * 1.9, 1.6);
  }
  const armPts = 400;
  for (const off of [0, Math.PI]) {
    for (let i = 0; i < armPts; i++) {
      const u = i / armPts;
      const t = 0.5 + u * 4.8;
      const r = (R * Math.pow(Math.E, 0.26 * t)) / Math.pow(Math.E, 0.26 * 5.3);
      if (r > R) continue;
      const jr = (rnd() - 0.5) * (0.3 + r * 0.26);
      const ja = (rnd() - 0.5) * (0.06 + u * 0.28);
      const wgt = 1.15 - u * 0.8;
      splat(
        cx + Math.cos(t + off + ja) * (r + jr),
        cy + Math.sin(t + off + ja) * (r + jr) * tilt,
        wgt,
      );
    }
  }
  for (let i = 0; i < 10; i++) splat(rnd() * w, rnd() * h, 0.8);
  return d;
}

const INK = (v: number) =>
  v >= 3.4 ? "●" : v >= 1.7 ? "•" : v >= 0.55 ? "·" : "";
const INK_COLOR = { "●": "#ffffff", "•": "#c8c8c8", "·": "#6f6f6f" } as const;

export function Galaxy({
  width = 40,
  height = 13,
  seed = 7,
}: {
  width?: number;
  height?: number;
  seed?: number;
}) {
  const grid = React.useMemo(() => galaxy(width, height, seed), [width, height, seed]);
  return (
    <>
      {grid.map((row, i) => {
        // group consecutive same-glyph cells into runs
        const runs: { g: string; text: string }[] = [];
        for (const v of row) {
          const g = INK(v);
          const ch = g || " ";
          const last = runs[runs.length - 1];
          if (last && last.g === ch) last.text += ch;
          else runs.push({ g: ch, text: ch });
        }
        return (
          <Text key={i}>
            {runs.map((r, j) => (
              <Text
                key={j}
                color={(INK_COLOR as any)[r.g] ?? "#000000"}
              >
                {r.text}
              </Text>
            ))}
          </Text>
        );
      })}
    </>
  );
}
