import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

export function afterWrite(workspace: string, file: string): string | null {
  const ext = path.extname(file);
  const run = (cmd: string, args: string[]) => {
    const o = spawnSync(cmd, args, { cwd: workspace, encoding: "utf8", timeout: 8000, windowsHide: true });
    const s = `${o.stdout ?? ""}${o.stderr ?? ""}`.trim();
    return s ? s.slice(0, 4000) : null;
  };
  try {
    if (ext === ".rs" && fs.existsSync(path.join(workspace, "Cargo.toml"))) {
      return run("cargo", ["check", "-q"]);
    }
    if ((ext === ".ts" || ext === ".tsx" || ext === ".js") && fs.existsSync(path.join(workspace, "tsconfig.json"))) {
      return run("tsc", ["--noEmit", "--pretty", "false"]);
    }
    if (ext === ".go") return run("go", ["build", "./..."]);
  } catch {
    return null;
  }
  return null;
}
