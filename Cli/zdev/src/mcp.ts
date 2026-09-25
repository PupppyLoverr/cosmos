// Minimal MCP stdio client (contract §11): initialize, tools/list,
// tools/call — the same shape as zcli's ztools/src/mcp.rs. Servers from
// config.toml [mcp.<name>] are spawned per call and killed afterwards.
import { spawn } from "node:child_process";

type Server = { command: string; args?: string[]; env?: Record<string, string> };

async function withServer<T>(
  servers: Record<string, Server>,
  name: string,
  f: (rpc: (method: string, params: any) => Promise<any>) => Promise<T>,
  signal?: AbortSignal,
): Promise<T> {
  const cfg = servers[name];
  if (!cfg?.command) throw new Error(`unknown MCP server ${name}`);
  const child = spawn(cfg.command, cfg.args ?? [], {
    stdio: ["pipe", "pipe", "ignore"],
    env: { ...process.env, ...(cfg.env ?? {}) },
    windowsHide: true,
    shell: process.platform === "win32" && !/\.(exe|com)$/i.test(cfg.command),
  });
  let buf = "";
  let seq = 0;
  const waiting = new Map<number, (v: any) => void>();
  child.stdout!.on("data", (d) => {
    buf += d.toString("utf8");
    let i: number;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i).trim();
      buf = buf.slice(i + 1);
      if (!line) continue;
      try {
        const v = JSON.parse(line);
        const w = typeof v.id === "number" ? waiting.get(v.id) : undefined;
        if (w) {
          waiting.delete(v.id);
          w(v);
        }
      } catch {}
    }
  });
  const dead = new Promise<never>((_, rej) => {
    child.on("error", (e) => rej(new Error(`spawn mcp server ${name}: ${e.message}`)));
    child.on("exit", (c) => rej(new Error(`mcp server ${name} exited (${c})`)));
    signal?.addEventListener("abort", () => rej(new Error("interrupted")));
  });
  const rpc = (method: string, params: any): Promise<any> => {
    const id = ++seq;
    const reply = new Promise<any>((res) => waiting.set(id, res));
    child.stdin!.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    const timeout = new Promise<never>((_, rej) => setTimeout(() => rej(new Error(`mcp ${method} timed out`)), 60_000));
    return Promise.race([reply, dead, timeout]).then((v) => {
      if (v.error) throw new Error(`mcp error: ${JSON.stringify(v.error)}`);
      return v.result;
    });
  };
  try {
    await rpc("initialize", {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "zdev", version: "0.1.0" },
    }).catch(() => {});
    child.stdin!.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
    return await f(rpc);
  } finally {
    dead.catch(() => {});
    try {
      child.kill();
    } catch {}
  }
}

export async function run(
  servers: Record<string, Server>,
  name: string,
  input: Record<string, any>,
  signal?: AbortSignal,
): Promise<string> {
  if (!Object.keys(servers).length) throw new Error("no MCP servers configured");
  if (name === "tool_search") {
    const q = String(input.query ?? "").toLowerCase();
    let hits = "";
    for (const server of Object.keys(servers)) {
      try {
        const res = await withServer(servers, server, (rpc) => rpc("tools/list", {}), signal);
        for (const t of res?.tools ?? []) {
          const n = String(t.name ?? "");
          const d = String(t.description ?? "");
          if (!q || n.toLowerCase().includes(q) || d.toLowerCase().includes(q)) {
            hits += `${server}.${n} — ${d}\n${JSON.stringify(t.inputSchema ?? {})}\n`;
          }
        }
      } catch {}
    }
    return hits || "no MCP tools matched";
  }
  const res = await withServer(
    servers,
    String(input.server ?? ""),
    (rpc) => rpc("tools/call", { name: input.tool, arguments: input.arguments ?? {} }),
    signal,
  );
  return JSON.stringify(res);
}
