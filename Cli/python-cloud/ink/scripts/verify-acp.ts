// Dev harness: drives AcpClient through init → new → prompt(tool:bash) →
// permission deny → prompt(ping), printing the update stream.
//   COSMOS_CLI_HOME=<fixture home> bun run scripts/verify-acp.ts
import { AcpClient } from "../src/acp";

const c = new AcpClient(process.cwd());
const updates: any[] = [];

c.onUpdate = (_sid, u) => {
  updates.push(u);
  const tag =
    u.sessionUpdate === "agent_message_chunk"
      ? `chunk ${JSON.stringify(u.content?.text)}`
      : u.sessionUpdate === "tool_call"
        ? `tool ${u.title} ${JSON.stringify(u.input)}`
        : u.sessionUpdate === "tool_call_update"
          ? `update ${u.status} ${JSON.stringify(text(u)).slice(0, 80)}`
          : JSON.stringify(u);
  console.log("  upd:", tag);
};
function text(u: any) {
  return (u.content ?? []).map((b: any) => b.content?.text ?? "").join("\n");
}

c.onRequest = (req) => {
  console.log("  req:", req.method, req.params?.tool, "→ deny");
  c.reply(req.id, {
    outcome: { outcome: "selected", optionId: "deny" },
  });
};

const init: any = await c.call("initialize", {});
console.log("init:", init.result.agentInfo);
const sn: any = await c.call("session/new", {
  cwd: process.cwd(),
  model: "fixture",
});
const sid = sn.result.sessionId;
console.log("session:", sid, sn.result.model, sn.result.mode);

const pr: any = await c.call("session/prompt", {
  sessionId: sid,
  prompt: [{ type: "text", text: 'tool:bash {"command":"echo hi"}' }],
});
console.log("stopReason:", pr.result.stopReason);

const pr2: any = await c.call("session/prompt", {
  sessionId: sid,
  prompt: [{ type: "text", text: "ping" }],
});
console.log("stopReason:", pr2.result.stopReason);

const ok =
  pr.result.stopReason === "end_turn" &&
  updates.some((u) => u.sessionUpdate === "tool_call" && u.title === "bash") &&
  updates.some(
    (u) => u.sessionUpdate === "tool_call_update" && u.status === "failed",
  ) &&
  updates.some(
    (u) => u.sessionUpdate === "agent_message_chunk" && /pong/.test(u.content?.text ?? ""),
  );
console.log(ok ? "OK verify-acp" : "FAIL verify-acp");
c.kill();
process.exit(ok ? 0 : 1);
