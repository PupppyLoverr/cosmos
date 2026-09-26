import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { cliHome } from "./proto";
import type { World } from "./world";
import { render } from "./world";

const FORBIDDEN = ["CONTRACT.md", "zsandbox", "auth_cmd.rs", "creds.rs", "oauth.ts", "wincred.ts"];

export function forbiddenPath(p: string): boolean {
  return FORBIDDEN.some((f) => p.includes(f));
}

export function skillify(name: string, world: World, draft: boolean, home = cliHome(), allowSelfMutate = false): string {
  if (!allowSelfMutate && forbiddenPath(name)) throw new Error("self-improve refused: target is fenced");
  const dir = path.join(home, "skills", draft ? "drafts" : "", name);
  const dest = path.join(dir, "SKILL.md");
  if (!draft && fs.existsSync(dest)) throw new Error(`skill ${name} exists — use /refine`);
  fs.mkdirSync(dir, { recursive: true });
  const body = `---\nname: ${name}\ndescription: "${world.goal.slice(0, 120).replace(/"/g, "'")}"\n---\n\n# ${name}\n\n## Goal\n${world.goal}\n\n## World\n${render(world)}\n`;
  if (/sk-.*api|begin private key|aws_secret/i.test(body)) throw new Error("self-improve refused: secret scanner");
  fs.writeFileSync(dest, body);
  return dest;
}

export function eventData(surface: string, detail: unknown): Record<string, unknown> {
  return { surface, detail };
}

export function correctionRow(command: string, fix: string): string {
  return `command \`${command}\` failed twice; fix: ${fix}`;
}

function meridian(args: string[]): string {
  const mer =
    process.env.MERIDIAN_BIN ||
    ["meridian.exe", "meridian"].map((n) => path.join(path.dirname(process.execPath), n)).find((p) => fs.existsSync(p));
  if (!mer) throw new Error("meridian bridge binary not found (set MERIDIAN_BIN)");
  const out = spawnSync(mer, args, { encoding: "utf8", windowsHide: true });
  if (out.status !== 0) throw new Error(`meridian ${args[0]}: ${(out.stderr ?? "").trim()}`);
  return (out.stdout ?? "").trim();
}

export function refinePropose(target: string, diff: string, evidence = ""): string {
  return meridian(["refine-propose", "--target", target, "--diff", diff, "--evidence", evidence]);
}
