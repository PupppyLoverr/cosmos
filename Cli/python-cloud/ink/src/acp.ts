// AcpClient — line-delimited JSON-RPC over stdio to `zdev acp`.
// Handles client→server calls, session/update notifications, and
// server→client requests (session/request_permission).
import { spawn, type ChildProcess } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SRC_DIR = path.dirname(fileURLToPath(import.meta.url));
// ink/src/acp.ts → python-cloud/ is two levels up from src/
export const PY_PROJECT = path.resolve(SRC_DIR, "..", "..");
export const INK_DIR = path.resolve(SRC_DIR, "..");

export type ServerRequest = {
  id: number;
  method: string;
  params: any;
};

type Pending = { res: (v: any) => void; rej: (e: Error) => void };

export class AcpClient {
  private child: ChildProcess;
  private buf = "";
  private seq = 1;
  private pending = new Map<number, Pending>();
  stderrTail: string[] = [];

  onUpdate: (sessionId: string, update: any) => void = () => {};
  onRequest: (req: ServerRequest) => void = () => {};
  onExit: (code: number | null) => void = () => {};

  constructor(workspace: string) {
    // ZDEV_ACP_CMD overrides the launcher entirely ("uv run -q zdev", etc.).
    const custom = process.env.ZDEV_ACP_CMD?.split(" ").filter(Boolean);
    const [cmd, args] = custom?.length
      ? [custom[0], [...custom.slice(1), "acp"]]
      : ["uv", ["run", "-q", "--project", PY_PROJECT, "zdev", "acp"]];
    this.child = spawn(cmd, args, {
      cwd: workspace,
      stdio: ["pipe", "pipe", "pipe"],
      env: process.env,
    });
    this.child.stdout!.setEncoding("utf8");
    this.child.stdout!.on("data", (d: string) => this.feed(d));
    this.child.stderr!.setEncoding("utf8");
    this.child.stderr!.on("data", (d: string) => {
      for (const l of d.split("\n")) {
        if (l.trim()) this.stderrTail.push(l.trim());
      }
      if (this.stderrTail.length > 50) this.stderrTail.splice(0, 25);
    });
    this.child.on("exit", (code) => {
      for (const [, p] of this.pending)
        p.rej(new Error(`acp process exited (${code})`));
      this.pending.clear();
      this.onExit(code);
    });
  }

  private feed(d: string) {
    this.buf += d;
    let i: number;
    while ((i = this.buf.indexOf("\n")) >= 0) {
      const line = this.buf.slice(0, i).trim();
      this.buf = this.buf.slice(i + 1);
      if (!line) continue;
      let msg: any;
      try {
        msg = JSON.parse(line);
      } catch {
        continue;
      }
      if (msg.method === "session/update") {
        this.onUpdate(msg.params?.sessionId ?? "", msg.params?.update);
      } else if (msg.method != null) {
        this.onRequest({ id: msg.id, method: msg.method, params: msg.params });
      } else if (msg.id != null) {
        const p = this.pending.get(msg.id);
        if (p) {
          this.pending.delete(msg.id);
          p.res(msg);
        }
      }
    }
  }

  call(method: string, params: any): Promise<any> {
    const id = this.seq++;
    return new Promise((res, rej) => {
      this.pending.set(id, { res, rej });
      this.child.stdin!.write(
        JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n",
      );
    });
  }

  notify(method: string, params: any) {
    this.child.stdin!.write(
      JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n",
    );
  }

  reply(id: number, result: any) {
    this.child.stdin!.write(
      JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n",
    );
  }

  kill() {
    try {
      this.child.stdin!.end();
    } catch {}
    try {
      this.child.kill();
    } catch {}
  }
}
