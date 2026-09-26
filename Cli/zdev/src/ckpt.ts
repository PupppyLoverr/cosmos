import fs from "node:fs";
import path from "node:path";
import { cliHome } from "./proto";

export function ckptDir(session: string, home = cliHome()): string {
  return path.join(home, "ckpt", session);
}

export function save(session: string, file: string, home = cliHome()): string | null {
  if (!fs.existsSync(file) || !fs.statSync(file).isFile()) return null;
  const dir = ckptDir(session, home);
  fs.mkdirSync(dir, { recursive: true });
  const seq =
    fs
      .readdirSync(dir)
      .map((n) => parseInt(n.split("-")[0], 10))
      .filter((n) => !Number.isNaN(n))
      .reduce((a, b) => Math.max(a, b), 0) + 1;
  const base = path.basename(file);
  const dest = path.join(dir, `${seq}-${base}`);
  fs.copyFileSync(file, dest);
  fs.writeFileSync(path.join(dir, `${seq}.meta`), file);
  return dest;
}

export function restoreLast(session: string, home = cliHome()): string {
  const dir = ckptDir(session, home);
  const metas = fs
    .readdirSync(dir)
    .filter((n) => n.endsWith(".meta"))
    .map((n) => ({ seq: parseInt(n, 10), p: path.join(dir, n) }))
    .sort((a, b) => a.seq - b.seq);
  const last = metas.pop();
  if (!last) throw new Error("no checkpoints");
  const target = fs.readFileSync(last.p, "utf8");
  const src = path.join(dir, `${last.seq}-${path.basename(target)}`);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.copyFileSync(src, target);
  fs.unlinkSync(src);
  fs.unlinkSync(last.p);
  return target;
}
