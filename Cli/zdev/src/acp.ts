// ACP stdio JSON-RPC (contract §11) — port of the Python `_acp` server.
//
// Concurrency model: a stdin reader routes
//   · replies (no `method`, `id` matching `pending`) → the waiting request()
//   · `session/cancel` → a per-session flag (acts even mid-prompt)
//   · everything else → a serial handler queue
// so a server→client request (session/request_permission) can be answered
// while a prompt turn is still running.
import { Agent, runHooks } from "./agent";
import * as proto from "./proto";
import * as store from "./store";

export type Json = Record<string, any>;

export function updateFor(sid: string, ev: proto.Event): Json | null {
  const k = ev.kind;
  const d = ev.data;
  let u: Json;
  if (k === "assistant") {
    u = {
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: d.text ?? "" },
    };
  } else if (k === "tool_call") {
    u = {
      sessionUpdate: "tool_call",
      toolCallId: d.id ?? "",
      title: d.name ?? "",
      input: d.input ?? {},
    };
  } else if (k === "tool_result") {
    u = {
      sessionUpdate: "tool_call_update",
      toolCallId: d.tool_use_id ?? "",
      title: d.name ?? "",
      status: d.is_error ? "failed" : "completed",
      content: [
        { type: "content", content: { type: "text", text: d.content ?? "" } },
      ],
    };
  } else if (k === "usage") {
    u = {
      sessionUpdate: "usage",
      input: d.input ?? 0,
      output: d.output ?? 0,
      cache_read: d.cache_read ?? 0,
      cache_write: d.cache_write ?? 0,
      usd: d.usd ?? null,
      cum_usd: d.cum_usd ?? null,
    };
  } else if (k === "plan") {
    u = { sessionUpdate: "plan", plan: d.plan ?? "" };
  } else if (k === "mode") {
    u = { sessionUpdate: "mode", from: d.from ?? "", mode: d.to ?? "" };
  } else if (k === "permission_decision") {
    u = {
      sessionUpdate: "notice",
      text:
        (d.granted ? "permission granted: " : "permission denied: ") +
        (d.tool ?? d.capability ?? ""),
    };
  } else if (k === "subagent_start") {
    u = {
      sessionUpdate: "notice",
      text: `⎇ subagent: ${String(d.prompt ?? "").slice(0, 120)}`,
    };
  } else if (k === "subagent_end") {
    u = {
      sessionUpdate: "notice",
      text: `⎇ subagent done: ${String(d.child_session ?? "").slice(0, 8)}`,
    };
  } else if (k === "compaction") {
    u = {
      sessionUpdate: "notice",
      text: `context compacted: ${d.before_tokens ?? "?"} → ${d.after_tokens ?? "?"} est. tokens`,
    };
  } else {
    return null;
  }
  return {
    jsonrpc: "2.0",
    method: "session/update",
    params: { sessionId: sid, update: u },
  };
}

async function* stdinLines(): AsyncGenerator<string> {
  let buf = "";
  for await (const chunk of process.stdin) {
    buf += chunk.toString("utf8");
    let i: number;
    while ((i = buf.indexOf("\n")) >= 0) {
      yield buf.slice(0, i);
      buf = buf.slice(i + 1);
    }
  }
  if (buf.trim()) yield buf;
}

export async function acpServer(): Promise<number> {
  const agents = new Map<string, Agent>();
  const cancels = new Map<string, { set: boolean }>();
  const pending = new Map<number, (msg: Json) => void>();
  let reqId = 10_000;
  let writeChain: Promise<void> = Promise.resolve();

  const send = (v: Json) => {
    const line = JSON.stringify(v) + "\n";
    writeChain = writeChain.then(
      () =>
        new Promise<void>((res) =>
          process.stdout.write(line, () => res()),
        ),
    );
    return writeChain;
  };

  const request = (method: string, params: Json, timeoutMs = 600_000) =>
    new Promise<Json | null>((res) => {
      const rid = reqId++;
      const timer = setTimeout(() => {
        pending.delete(rid);
        res(null);
      }, timeoutMs);
      pending.set(rid, (msg) => {
        clearTimeout(timer);
        pending.delete(rid);
        res(msg);
      });
      void send({ jsonrpc: "2.0", id: rid, method, params });
    });

  const makeEmit = (sid: string) => (ev: proto.Event) => {
    const n = updateFor(sid, ev);
    if (n) void send(n);
  };

  // askerCtx holds the tool_call id/input of the in-flight exec_tool call so
  // the asker (which only receives the tool name) can report them accurately.
  const makeAsker =
    (a: Agent, sid: string, askerCtx: Json) => async (tool: string) => {
      const resp = await request("session/request_permission", {
        sessionId: sid,
        toolCallId: askerCtx.tid ?? tool,
        tool,
        input: askerCtx.input ?? {},
        options: [
          { id: "allow", name: "Allow once" },
          { id: "allow_always", name: "Always allow" },
          { id: "deny", name: "Deny" },
        ],
      });
      const outcome = resp?.result?.outcome ?? {};
      const opt = outcome.optionId ?? outcome.outcome ?? "deny";
      if (opt === "allow_always" || opt === "always") {
        if (!a.opts.allow.includes(tool)) a.opts.allow.push(tool);
        return true;
      }
      return opt === "allow" || opt === "selected" || opt === "yes";
    };

  const runTurns = async (
    a: Agent,
    sid: string,
    emit: (ev: proto.Event) => void,
    askerCtx: Json,
  ): Promise<string> => {
    // Provider turn + tool-exec loop. Returns an ACP stopReason.
    let cancel = cancels.get(sid);
    if (!cancel) {
      cancel = { set: false };
      cancels.set(sid, cancel);
    }
    for (let i = 0; i < (a.opts.max_turns ?? 16); i++) {
      if (cancel.set) return "cancelled";
      let done: boolean;
      try {
        done = await a.turn(emit);
      } catch {
        return "refusal";
      }
      if (done) return "end_turn";
      const calls = (a.messages[a.messages.length - 1].content ?? []).filter(
        (b) => b.type === "tool_use",
      );
      if (!calls.length) return "end_turn";
      for (const c of calls) {
        if (cancel.set) return "cancelled";
        askerCtx.tid = c.id;
        askerCtx.input = c.input;
        await a.execTool(c.id, c.name, c.input, emit);
        if (c.name === "submit_plan") return "end_turn"; // waits for session/approve
      }
    }
    return "max_turns";
  };

  const handle = async (req: Json) => {
    const method = req.method ?? "";
    const rid = req.id;
    const params = req.params ?? {};
    const result = (r: Json) => send({ jsonrpc: "2.0", id: rid, result: r });
    const err = (code: number, msg: string) =>
      send({ jsonrpc: "2.0", id: rid, error: { code, message: msg } });

    if (method === "initialize") {
      await result({
        protocolVersion: 1,
        agentInfo: { name: "zdev", version: "0.1.0" },
        agentCapabilities: {
          loadSession: false,
          promptCapabilities: { embeddedContext: true },
        },
      });
    } else if (method === "session/new") {
      const cwd = params.cwd ?? process.cwd();
      const o = new proto.RunOpts();
      o.cwd = cwd;
      if (["plan", "agent", "yolo"].includes(params.mode)) {
        o.mode = params.mode;
        o.yolo = params.mode === "yolo";
      }
      if (params.model) o.model = params.model;
      if (params.resume) o.resume = params.resume;
      let a: Agent;
      try {
        a = new Agent(o, "zdev", cwd);
      } catch (e: any) {
        await err(-32603, String(e?.message ?? e));
        return;
      }
      const sid = a.session.id;
      agents.set(sid, a);
      cancels.set(sid, { set: false });
      const askerCtx: Json = {};
      a.asker = makeAsker(a, sid, askerCtx);
      (a as any)._askerCtx = askerCtx;
      await result({ sessionId: sid, model: a.modelAlias, mode: a.mode });
    } else if (method === "session/prompt") {
      const sid = params.sessionId ?? "";
      const a = agents.get(sid);
      if (!a) {
        await err(-32602, "unknown sessionId");
        return;
      }
      const cancel = cancels.get(sid) ?? { set: false };
      cancels.set(sid, cancel);
      cancel.set = false;
      const prompt = (params.prompt ?? [])
        .map((b: Json) => b.text ?? "")
        .join("\n");
      const emit = makeEmit(sid);
      a.sendUser(prompt, emit);
      const reason = await runTurns(a, sid, emit, (a as any)._askerCtx ?? {});
      await result({ stopReason: reason });
    } else if (method === "session/approve") {
      // plan-mode approval: replays approve_plan + keeps running turns
      const sid = params.sessionId ?? "";
      const a = agents.get(sid);
      if (!a) {
        await err(-32602, "unknown sessionId");
        return;
      }
      const cancel = cancels.get(sid) ?? { set: false };
      cancels.set(sid, cancel);
      cancel.set = false;
      const emit = makeEmit(sid);
      a.approvePlan(emit);
      const reason = await runTurns(a, sid, emit, (a as any)._askerCtx ?? {});
      await result({ stopReason: reason });
    } else if (method === "session/set_mode") {
      const sid = params.sessionId ?? "";
      const a = agents.get(sid);
      const mode = params.mode ?? "";
      if (!a) {
        await err(-32602, "unknown sessionId");
      } else if (!["plan", "agent", "yolo"].includes(mode)) {
        await err(-32602, `bad mode ${JSON.stringify(mode)} (plan|agent|yolo)`);
      } else {
        const old = a.mode;
        a.mode = mode;
        a.opts.yolo = mode === "yolo";
        a.session.append("mode", { from: old, to: mode });
        await result({ mode });
      }
    } else if (method === "session/set_model") {
      // in-place provider swap: session log + messages are untouched,
      // the next turn() simply speaks to the new provider.
      const sid = params.sessionId ?? "";
      const a = agents.get(sid);
      const alias = params.model ?? "";
      if (!a) {
        await err(-32602, "unknown sessionId");
        return;
      }
      const found = store.providerFor(a.cfg, alias);
      if (!found) {
        await err(-32602, `unknown model ${JSON.stringify(alias)}`);
        return;
      }
      const [provider, modelId] = found;
      a.provider = provider;
      a.modelAlias = `${alias} (${modelId})`;
      // contract §6 kind is `model_switch` (validator rejects `model`).
      a.session.append("model_switch", { alias, id: modelId });
      await result({ model: a.modelAlias });
    } else if (method === "session/list_models") {
      const cfg = store.loadConfig(proto.cliHome());
      const models = Object.keys(cfg.models)
        .sort()
        .map((k) => {
          const m = cfg.models[k];
          return {
            alias: k,
            provider: m.provider ?? "",
            id: m.id ?? "",
            context: m.context ?? 0,
            free: !Object.values(m.price ?? {}).some((v) => v),
          };
        });
      await result({ models, default: cfg.default_model });
    } else if (rid !== undefined && rid !== null) {
      await err(-32601, "method not found");
    }
  };

  // serial handler queue — responses stay ordered, the reader never blocks
  let queue: Promise<void> = Promise.resolve();
  const enqueue = (msg: Json) => {
    queue = queue.then(() => handle(msg)).catch((e) => {
      console.error(`acp handler: ${e?.message ?? e}`);
    });
  };

  for await (const raw of stdinLines()) {
    const line = raw.trim();
    if (!line) continue;
    let msg: Json;
    try {
      msg = JSON.parse(line);
    } catch {
      continue;
    }
    if (msg.method === undefined || msg.method === null) {
      // reply to our request
      const p = pending.get(msg.id);
      if (p) p(msg);
      continue;
    }
    if (msg.method === "session/cancel") {
      const sid = msg.params?.sessionId ?? "";
      const ev = cancels.get(sid);
      if (ev) ev.set = true;
      if (msg.id !== undefined && msg.id !== null) {
        void send({ jsonrpc: "2.0", id: msg.id, result: { cancelled: true } });
      }
      continue;
    }
    enqueue(msg);
  }
  await queue;
  return proto.EXIT_DONE;
}
