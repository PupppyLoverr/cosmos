#!/usr/bin/env node
// ACP smoke test: spawn `<bin> acp`, do initialize → session/new →
// session/prompt "ping", assert session/update notifications + stopReason.
// Usage: node eval/acp-smoke.mjs <path-to-exe-or-launcher> [args...]
import { spawn } from "node:child_process";

const [bin, ...binArgs] = process.argv.slice(2);
const args = [...binArgs, "acp"];
const spawnCwd = process.env.ACP_CWD || process.cwd();
const child = spawn(bin, args, { cwd: spawnCwd, stdio: ["pipe", "pipe", "ignore"] });

let buf = "";
const pending = new Map();
const updates = [];
child.stdout.on("data", (d) => {
  buf += d.toString();
  let i;
  while ((i = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, i).trim();
    buf = buf.slice(i + 1);
    if (!line) continue;
    const msg = JSON.parse(line);
    if (msg.method === "session/update") updates.push(msg.params.update);
    else if (msg.id != null) pending.get(msg.id)?.(msg);
  }
});

const call = (id, method, params) =>
  new Promise((res) => {
    pending.set(id, res);
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  });

const init = await call(1, "initialize", {});
if (init.result?.protocolVersion !== 1) {
  console.error("FAIL initialize", JSON.stringify(init));
  process.exit(1);
}
const sn = await call(2, "session/new", { cwd: process.cwd() });
const sid = sn.result?.sessionId;
if (!sid) {
  console.error("FAIL session/new", JSON.stringify(sn));
  process.exit(1);
}
const pr = await call(3, "session/prompt", {
  sessionId: sid,
  prompt: [{ type: "text", text: "ping" }],
});
const text = updates
  .filter((u) => u.sessionUpdate === "agent_message_chunk")
  .map((u) => u.content?.text ?? "")
  .join("");
if (!text.includes("pong")) {
  console.error("FAIL: no pong in updates", JSON.stringify(updates));
  process.exit(1);
}
if (pr.result?.stopReason !== "end_turn") {
  console.error("FAIL stopReason", JSON.stringify(pr));
  process.exit(1);
}
console.log(`OK acp ${bin} sid=${sid} updates=${updates.length}`);
child.stdin.end();
child.kill();
process.exit(0);
