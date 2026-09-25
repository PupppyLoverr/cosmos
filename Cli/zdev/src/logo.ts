// The Cosmos mark — the same black-and-white Milky Way zcli draws
// (rust-tui/crates/ztui/src/logo.rs), ported number for number: a tilted
// two-arm logarithmic spiral with a bright core and a few field stars,
// sampled on a 2×4-dots-per-cell braille grid and shaded by the light each
// cell holds. At 12×4 cells it is identical to zcli's; larger sizes sample
// the same galaxy more finely.

/** Cheap integer hash → [0, 1), bit-identical to logo.rs `noise`. */
function noise(x: number, y: number, salt: number): number {
  let h = (Math.imul(x, 0x9e3779b1) ^ Math.imul(y, 0x85ebca77) ^ salt) >>> 0;
  h = (h ^ (h >>> 15)) >>> 0;
  h = Math.imul(h, 0x2c1b3c6d) >>> 0;
  h = (h ^ (h >>> 12)) >>> 0;
  h = Math.imul(h, 0x297a2d39) >>> 0;
  h = (h ^ (h >>> 15)) >>> 0;
  return (h & 0xffff) / 65536;
}

/** Light at one dot of a W×H dot grid (0 = empty sky). */
function light(x: number, y: number, W: number, H: number): number {
  const cx = (W - 1) / 2;
  const cy = (H - 1) / 2;
  const dx = (x - cx) / (W / 2);
  const dy = (y - cy) / (H / 2);
  // tilt the disk, then undo the viewing inclination
  const a = -0.4;
  const u = dx * Math.cos(a) + dy * Math.sin(a);
  const v = (-dx * Math.sin(a) + dy * Math.cos(a)) / 0.85;
  const r = Math.sqrt(u * u + v * v);
  const theta = Math.atan2(v, u);
  const core = Math.exp(-((r / 0.25) ** 2)) * 1.6;
  // two arms wound as a logarithmic spiral
  const wind = theta - 3 * Math.log(r + 0.05);
  const arm = ((1 + Math.cos(2 * wind)) / 2) ** 1.5;
  const disk = r < 1.05 ? Math.exp(-(r / 0.9)) : 0;
  const dust = arm * disk * 2.2 + disk * 0.18;
  const stars = noise(x, y, 7) > 0.975 && r > 0.8 ? 0.8 : 0;
  return core + dust + stars;
}

export type LogoCell = { ch: string; lum: number };

const BIT = [
  [0x01, 0x02, 0x04, 0x40],
  [0x08, 0x10, 0x20, 0x80],
];

const cache = new Map<string, LogoCell[][]>();

/** The galaxy as `rows` × `cols` braille cells. */
export function logoGrid(cols = 12, rows = 4): LogoCell[][] {
  const key = `${cols}x${rows}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const W = cols * 2;
  const H = rows * 4;
  const out: LogoCell[][] = [];
  for (let cy = 0; cy < rows; cy++) {
    const row: LogoCell[] = [];
    for (let cx = 0; cx < cols; cx++) {
      let bits = 0;
      let sum = 0;
      let on = 0;
      for (let ix = 0; ix < 2; ix++) {
        for (let iy = 0; iy < 4; iy++) {
          const x = cx * 2 + ix;
          const y = cy * 4 + iy;
          const l = light(x, y, W, H);
          // dither: jitter the threshold so edges break up into dust
          if (l + (noise(x, y, 31) - 0.5) * 0.35 > 0.34) {
            bits |= BIT[ix][iy];
            sum += l;
            on++;
          }
        }
      }
      row.push({
        ch: bits ? String.fromCharCode(0x2800 + bits) : " ",
        lum: on ? sum / on : 0,
      });
    }
    out.push(row);
  }
  cache.set(key, out);
  return out;
}

/** Hex colour for a cell's luminance — core white → arms grey → dust dim. */
export function logoShade(lum: number): { color: string; bold: boolean } {
  if (lum > 1.0) return { color: "#f4f4f4", bold: true };
  if (lum > 0.55) return { color: "#c4c4c4", bold: false };
  return { color: "#7c7c7c", bold: false };
}
