// The launch sound — the same soft galaxy bloom zcli plays, sample for
// sample (zwelcome `synth`): a low swell as the galaxy comes into view, air
// that opens up as you fly through it, a soft burst into dust at ~2.9 s,
// pentatonic pings as the letters land and a quiet chord under the title.
// Rendered to a 16-bit WAV and played by the OS player in the background;
// `stop()` cuts it when the intro is skipped. ZDEV_QUIET / ZCLI_QUIET mute.
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const SOUND_SECS = 4.4;

const smooth01 = (x: number) => {
  x = Math.min(1, Math.max(0, x));
  return x * x * (3 - 2 * x);
};

export function synth(rate: number): Float32Array {
  const TAU = Math.PI * 2;
  const PINGS: [number, number][] = [
    [3.05, 784.0],
    [3.22, 987.77],
    [3.4, 1174.7],
    [3.62, 1568.0],
    [3.88, 2093.0],
  ];
  const CHORD = [261.63, 392.0, 523.25, 659.25];
  const n = Math.floor(rate * SOUND_SECS);
  const out = new Float32Array(n);
  let rng = 0x243f6a8885a308d3n;
  const MASK = (1n << 64n) - 1n;
  let lp = 0,
    lp2 = 0,
    sub = 0,
    boom = 0;
  for (let i = 0; i < n; i++) {
    const t = i / rate;
    rng ^= (rng << 13n) & MASK;
    rng ^= rng >> 7n;
    rng ^= (rng << 17n) & MASK;
    const white = Number(rng >> 40n) / 16_777_216 * 2 - 1;
    let s = 0;
    if (t < 1.6) {
      sub += (30 + 40 * Math.exp(-1.8 * t)) / rate;
      s += Math.sin(TAU * sub) * 0.28 * smooth01(t / 0.35) * Math.exp(-1.6 * t);
    }
    if (t < 3.1) {
      const u = t / 3.1;
      const cut = 180 + 5200 * u ** 3;
      lp += (1 - Math.exp((-TAU * cut) / rate)) * (white - lp);
      s += lp * 0.55 * u ** 2.2 * (1 - u) ** 0.5;
    }
    if (t >= 2.9) {
      const d = t - 2.9;
      boom += 48 / rate;
      s += Math.sin(TAU * boom) * 0.3 * smooth01(d / 0.03) * Math.exp(-3 * d);
      const cut = 200 + 1400 * Math.exp(-3.5 * d);
      lp2 += (1 - Math.exp((-TAU * cut) / rate)) * (white - lp2);
      s += lp2 * 0.45 * smooth01(d / 0.02) * Math.exp(-4 * d);
    }
    for (const [pt, pf] of PINGS) {
      if (t >= pt) {
        const d = t - pt;
        s += Math.sin(TAU * pf * d) * 0.07 * Math.exp(-6 * d);
      }
    }
    if (t >= 3.0) {
      const d = t - 3.0;
      const env = smooth01(d / 0.5) * smooth01((SOUND_SECS - t) / 0.9);
      CHORD.forEach((f, k) => (s += Math.sin(TAU * f * d + (k * Math.PI) / 3) * 0.022 * env));
    }
    const fade = smooth01(t / 0.02) * smooth01((SOUND_SECS - t) / 0.3);
    out[i] = Math.tanh(s * fade * 1.4) * 0.85;
  }
  return out;
}

export function wav(samples: Float32Array, rate: number): Buffer {
  const data = Buffer.alloc(samples.length * 2);
  for (let i = 0; i < samples.length; i++) data.writeInt16LE(Math.round(Math.max(-1, Math.min(1, samples[i])) * 32767), i * 2);
  const h = Buffer.alloc(44);
  h.write("RIFF", 0);
  h.writeUInt32LE(36 + data.length, 4);
  h.write("WAVE", 8);
  h.write("fmt ", 12);
  h.writeUInt32LE(16, 16);
  h.writeUInt16LE(1, 20); // PCM
  h.writeUInt16LE(1, 22); // mono
  h.writeUInt32LE(rate, 24);
  h.writeUInt32LE(rate * 2, 28);
  h.writeUInt16LE(2, 32);
  h.writeUInt16LE(16, 34);
  h.write("data", 36);
  h.writeUInt32LE(data.length, 40);
  return Buffer.concat([h, data]);
}

let player: ChildProcess | null = null;
let file = "";

export function play(): void {
  if (process.env.ZDEV_QUIET || process.env.ZCLI_QUIET) return;
  try {
    const rate = 22_050;
    file = path.join(os.tmpdir(), `zdev-welcome-${process.pid}.wav`);
    fs.writeFileSync(file, wav(synth(rate), rate));
    const [cmd, args]: [string, string[]] =
      process.platform === "win32"
        ? ["powershell", ["-NoProfile", "-NonInteractive", "-Command", `(New-Object Media.SoundPlayer '${file}').PlaySync()`]]
        : process.platform === "darwin"
          ? ["afplay", [file]]
          : ["aplay", ["-q", file]];
    player = spawn(cmd, args, { stdio: "ignore", windowsHide: true });
    player.on("error", () => {});
    player.on("exit", cleanup);
  } catch {}
}

function cleanup() {
  player = null;
  if (file) {
    try {
      fs.unlinkSync(file);
    } catch {}
  }
}

/** Cut the sound (intro skipped or flow finished). */
export function stop(): void {
  if (!player) return;
  try {
    if (process.platform === "win32" && player.pid) spawn("taskkill", ["/pid", String(player.pid), "/t", "/f"], { stdio: "ignore", windowsHide: true });
    else player.kill();
  } catch {}
}
