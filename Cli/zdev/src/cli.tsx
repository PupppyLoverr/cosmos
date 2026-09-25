// zdev — Cosmos TypeScript/bun harness. `zdev -p "task"` headless,
// bare `zdev` launches the Ink TUI (line REPL when stdin isn't a TTY).
import fs from "node:fs";
import path from "node:path";

import * as proto from "./proto";
import * as store from "./store";
import * as agent from "./agent";
import * as t from "./tools";

export const HELP = `\
zdev — Cosmos coding-agent harness (TypeScript/bun)

usage:
  zdev                        interactive session (Ink TUI; line REPL on pipes)
  zdev -p "<task>" [flags]    headless run; -p - reads the task from stdin
  zdev ui                     launch the Ink TUI
  zdev acp                    ACP stdio JSON-RPC server
  zdev tools dump             canonical tools.json (byte-identical to zcli/zagy)
  zdev session list           sessions in COSMOS_CLI_HOME
  zdev session export <id> [--format md|jsonl]
  zdev session branch <id> --at <event_id>
  zdev session tree <id>
  zdev memory search <q>      search the shared Meridian vault
  zdev memory add <text>      append to the vault (scope: zdev)
  zdev login                  sign in with ChatGPT (subscription, PKCE)
  zdev logout                 remove stored ChatGPT tokens

flags: --output text|json|json-stream · --mode plan|agent · --allow a,b ·
--yolo · --no-sandbox · --model <alias> · --max-turns N · --max-usd F ·
--cwd <dir> · --resume <id> · --approve-plan · --allow-escalation cap,...
`;

// argv: `bun run src/cli.tsx` → [bun, cli.tsx, ...]; compiled exe → [exe, ...]
function argv(): string[] {
  const a = process.argv;
  const main = (Bun as any).main ?? "";
  const i = a.indexOf(main);
  if (i >= 0) return a.slice(i + 1);
  return a.slice(a.length > 1 && /\.tsx?$/.test(a[1]) ? 2 : 1);
}

function parse(args: string[]): proto.RunOpts {
  const o = new proto.RunOpts();
  const positional: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    const val = (): string => {
      i += 1;
      if (i >= args.length) throw new Error(`${a} needs a value`);
      return args[i];
    };
    if (a === "-p" || a === "--prompt") {
      o.prompt = val();
    } else if (a === "--output") {
      o.output = val();
      if (!["text", "json", "json-stream"].includes(o.output)) {
        throw new Error(`bad --output ${o.output}`);
      }
    } else if (a === "--mode") {
      o.mode = val();
    } else if (a === "--allow") {
      o.allow = val()
        .split(",")
        .map((x) => x.trim())
        .filter(Boolean);
    } else if (a === "--yolo") {
      o.yolo = true;
    } else if (a === "--no-sandbox") {
      o.no_sandbox = true;
    } else if (a === "--model") {
      o.model = val();
    } else if (a === "--max-turns") {
      o.max_turns = parseInt(val(), 10);
    } else if (a === "--max-usd") {
      o.max_usd = parseFloat(val());
    } else if (a === "--cwd") {
      o.cwd = val();
    } else if (a === "--resume") {
      o.resume = val();
    } else if (a === "--approve-plan") {
      o.approve_plan = true;
    } else if (a === "--allow-escalation") {
      o.allow_escalation = val()
        .split(",")
        .map((x) => x.trim())
        .filter(Boolean);
    } else if (["--version", "--help", "-h", "-V"].includes(a)) {
      process.stdout.write(`zdev ${proto.CONTRACT_VERSION} (contract v1)\n${HELP}`);
      process.exit(proto.EXIT_DONE);
    } else if (a.startsWith("-")) {
      throw new Error(`unknown flag ${a}`);
    } else {
      positional.push(a);
    }
  }
  if (o.prompt === null && positional.length) {
    o.prompt = positional.join(" ");
  }
  return o;
}

function render(ev: proto.Event): void {
  const k = ev.kind;
  const d = ev.data ?? {};
  if (k === "assistant") {
    if (d.text) console.log(d.text);
    for (const c of d.tool_calls ?? []) {
      console.error(
        `  → ${c.name} ${JSON.stringify(c.input ?? {}).slice(0, 120)}`,
      );
    }
  } else if (k === "tool_result") {
    const mark = d.is_error ? "✗" : "✓";
    console.error(`  ${mark} ${d.name}: ${String(d.content ?? "").slice(0, 160)}`);
  } else if (k === "plan") {
    console.log(
      `\n── plan ──\n${d.plan ?? ""}\n──────────\n(/approve to run, or keep refining)`,
    );
  } else if (k === "subagent_start") {
    console.error(`  ⎇ subagent: ${d.prompt ?? ""}`);
  }
}

// Shared stdin line queue for the REPL + the [y/N] asker.
class LineReader {
  private lines: string[] = [];
  private waiting: ((v: string | null) => void) | null = null;
  private ended = false;

  constructor() {
    let buf = "";
    process.stdin.on("data", (c) => {
      buf += c.toString("utf8");
      let i: number;
      while ((i = buf.indexOf("\n")) >= 0) {
        this.push(buf.slice(0, i).replace(/\r$/, ""));
        buf = buf.slice(i + 1);
      }
    });
    process.stdin.on("end", () => {
      if (buf) this.push(buf);
      this.ended = true;
      this.waiting?.(null);
    });
    process.stdin.resume();
  }

  private push(l: string) {
    if (this.waiting) {
      const w = this.waiting;
      this.waiting = null;
      w(l);
    } else {
      this.lines.push(l);
    }
  }

  next(): Promise<string | null> {
    if (this.lines.length) return Promise.resolve(this.lines.shift()!);
    if (this.ended) return Promise.resolve(null);
    return new Promise((res) => (this.waiting = res));
  }
}

async function runTurns(ag: agent.Agent): Promise<void> {
  for (let i = 0; i < (ag.opts.max_turns ?? 32); i++) {
    let done: boolean;
    try {
      done = await ag.turn(render);
    } catch (e: any) {
      console.error(`turn failed: ${e?.message ?? e}`);
      break;
    }
    if (done) break;
    const calls = ag.messages[ag.messages.length - 1].content.filter(
      (b) => b.type === "tool_use",
    );
    for (const b of calls) {
      await ag.execTool(b.id, b.name, b.input, render);
      if (b.name === "submit_plan") return; // wait for /approve
    }
  }
}

async function repl(opts: proto.RunOpts): Promise<number> {
  const ag = new agent.Agent(opts, "zdev", process.cwd());
  try {
    console.error(
      `zdev · ${ag.modelAlias} · mode ${ag.mode} · session ${ag.session.id.slice(0, 8)}`,
    );
    if (ag.ctx.sandboxDegraded) {
      console.error(`sandbox degraded: ${ag.ctx.sandboxDegraded}`);
    }
    const reader = new LineReader();
    ag.asker = async (tool: string) => {
      process.stderr.write(`allow ${tool}? [y/N] `);
      const line = await reader.next();
      if (line === null) return false;
      return ["y", "yes"].includes(line.trim().toLowerCase());
    };
    render(
      ag.session.append("session_start", {
        model: ag.modelAlias,
        mode: ag.mode,
        sandbox: ag.ctx.noSandbox ? "off" : "appcontainer",
        boot_tokens: ag.bootTokens,
      }),
    );
    if (opts.prompt && opts.prompt !== "-") {
      ag.sendUser(opts.prompt, render);
      await runTurns(ag);
    }
    while (true) {
      if (process.stdin.isTTY) process.stderr.write("› ");
      const line = await reader.next();
      if (line === null) break;
      const cmd = line.trim();
      if (!cmd) continue;
      if (["/quit", "/exit", "/q"].includes(cmd)) {
        break;
      } else if (cmd === "/plan") {
        ag.mode = "plan";
        console.error("mode: plan");
      } else if (cmd === "/agent") {
        ag.mode = "agent";
        console.error("mode: agent");
      } else if (cmd === "/yolo") {
        ag.mode = "yolo";
        console.error("mode: yolo");
      } else if (cmd === "/approve") {
        ag.approvePlan(render);
        await runTurns(ag);
      } else if (cmd === "/session") {
        console.error(`session ${ag.session.id}`);
      } else if (cmd === "/help") {
        console.error("commands: /plan /agent /yolo /approve /session /quit");
      } else {
        ag.sendUser(cmd, render);
        await runTurns(ag);
      }
    }
    ag.session.append("session_end", { reason: "quit" });
    return proto.EXIT_DONE;
  } finally {
    ag.cleanup();
  }
}

function sessionCmd(args: string[]): number {
  const home = proto.cliHome();
  const sub = args[0] ?? "";
  if (sub === "list") {
    for (const s of store.Session.list(home)) console.log(JSON.stringify(s));
    return proto.EXIT_DONE;
  }
  if (sub === "export") {
    const sid = args[1] ?? "";
    let fmt = "md";
    const i = args.indexOf("--format");
    if (i >= 0) fmt = args[i + 1] ?? "md";
    try {
      process.stdout.write(store.Session.export(home, sid, fmt));
      return proto.EXIT_DONE;
    } catch (e: any) {
      console.error(e?.message ?? e);
      return proto.EXIT_USAGE;
    }
  }
  if (sub === "branch") {
    const sid = args[1] ?? "";
    let at = "";
    const i = args.indexOf("--at");
    if (i >= 0) at = args[i + 1] ?? "";
    try {
      const s = store.Session.open(home, sid);
      s.branch(at);
      console.log(`branched ${sid} at ${at}`);
      return proto.EXIT_DONE;
    } catch (e: any) {
      console.error(e?.message ?? e);
      return proto.EXIT_USAGE;
    }
  }
  if (sub === "tree") {
    const sid = args[1] ?? "";
    let evs: proto.Event[];
    try {
      evs = store.Session.open(home, sid).events();
    } catch (e: any) {
      console.error(e?.message ?? e);
      return proto.EXIT_USAGE;
    }
    const byParent = new Map<string | null, proto.Event[]>();
    for (const e of evs) {
      const l = byParent.get(e.parent) ?? [];
      l.push(e);
      byParent.set(e.parent, l);
    }
    const walk = (parent: string | null, depth: number) => {
      for (const e of byParent.get(parent) ?? []) {
        console.log(`${"  ".repeat(depth)}${e.id} [${e.kind}] ${e.ts}`);
        walk(e.id, depth + 1);
      }
    };
    walk(null, 0);
    return proto.EXIT_DONE;
  }
  console.error("usage: zdev session list|export|branch|tree");
  return proto.EXIT_USAGE;
}

function memoryCmd(args: string[]): number {
  const ctx = new t.Ctx(process.cwd());
  const sub = args[0] ?? "";
  if (sub === "search") {
    console.log(t.run(ctx, "memory_search", { query: args.slice(1).join(" ") }));
    return proto.EXIT_DONE;
  }
  if (sub === "add") {
    let ty = "map";
    const words: string[] = [];
    for (let i = 1; i < args.length; i++) {
      if (args[i] === "--type") {
        ty = args[++i] ?? "map";
      } else {
        words.push(args[i]);
      }
    }
    console.log(
      t.run(ctx, "memory_write", {
        type: ty,
        text: words.join(" "),
        scope: "zdev",
      }),
    );
    return proto.EXIT_DONE;
  }
  console.error("usage: zdev memory search|add");
  return proto.EXIT_USAGE;
}

async function main(): Promise<number> {
  const args = argv();
  if (args[0] === "tools") {
    if (args[1] === "dump") {
      process.stdout.write(t.toolsDump(), "utf8");
      return proto.EXIT_DONE;
    }
    console.error("usage: zdev tools dump");
    return proto.EXIT_USAGE;
  }
  if (args[0] === "session") return sessionCmd(args.slice(1));
  if (args[0] === "memory") return memoryCmd(args.slice(1));
  if (args[0] === "login") {
    const { loginChatGPT } = await import("./oauth");
    try {
      const a = await loginChatGPT(proto.cliHome(), (u) =>
        console.error(`open this URL to sign in:\n${u}`),
      );
      console.error(`signed in${a.email ? ` as ${a.email}` : ""}`);
      return proto.EXIT_DONE;
    } catch (e: any) {
      console.error(`login failed: ${e?.message ?? e}`);
      return proto.EXIT_FAILED;
    }
  }
  if (args[0] === "logout") {
    const { clearAuth } = await import("./oauth");
    clearAuth(proto.cliHome());
    console.error("signed out");
    return proto.EXIT_DONE;
  }
  if (args[0] === "acp") {
    const { acpServer } = await import("./acp");
    return acpServer();
  }
  const uiOnly = args[0] === "ui" ? args.slice(1) : null;
  let opts: proto.RunOpts;
  try {
    opts = parse(uiOnly ?? args);
  } catch (e: any) {
    console.error(`${e?.message ?? e}\n${HELP}`);
    return proto.EXIT_USAGE;
  }
  try {
    if (opts.prompt !== null) {
      return await agent.runHeadless(opts, "zdev", process.cwd());
    }
    if (process.stdin.isTTY) {
      const { runTui } = await import("./ui/index");
      return await runTui(opts);
    }
    return await repl(opts);
  } catch (e: any) {
    console.error(String(e?.message ?? e));
    return proto.EXIT_FAILED;
  }
}

process.exit(await main());
