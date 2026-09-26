// Agent loop — same semantics as zcore::agent (contract §3-§9).
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import * as proto from "./proto";
import * as providers from "./providers";
import * as store from "./store";
import * as t from "./tools";
import * as compact from "./compact";
import * as repair from "./repair";
import * as world from "./world";
import * as improve from "./improve";
import * as verify from "./verify";
import * as ckpt from "./ckpt";

// CONTRACT.md next to the harness (Cli/CONTRACT.md from src/, repo checkout
// layout); embedded copy keeps the compiled binary honest when the file
// isn't reachable via import.meta.dir.
import CONTRACT_MD_TEXT from "../../CONTRACT.md" with { type: "text" };

const SRC_DIR = path.dirname(fileURLToPath(import.meta.url));

export function systemSnippet(): string {
  const candidates = [
    path.resolve(SRC_DIR, "..", "..", "CONTRACT.md"), // dev: zdev/src → Cli
    path.resolve(process.cwd(), "CONTRACT.md"),
  ];
  for (const p of candidates) {
    try {
      const text = fs.readFileSync(p, "utf8");
      const s = text.indexOf("<!-- BEGIN SYSTEM -->") + "<!-- BEGIN SYSTEM -->".length;
      const e = text.indexOf("<!-- END SYSTEM -->");
      if (0 < s && s < e) return text.slice(s, e).trim();
    } catch {}
  }
  {
    const text = CONTRACT_MD_TEXT;
    const s = text.indexOf("<!-- BEGIN SYSTEM -->") + "<!-- BEGIN SYSTEM -->".length;
    const e = text.indexOf("<!-- END SYSTEM -->");
    if (0 < s && s < e) return text.slice(s, e).trim();
  }
  return (
    "You are Cosmos, a coding agent running in the operator's terminal. " +
    "You work through tools."
  );
}

function findMeridian(): string | null {
  const v = process.env.MERIDIAN_BIN;
  if (v && fs.existsSync(v) && fs.statSync(v).isFile()) return v;
  for (const d of (process.env.PATH ?? "").split(path.delimiter)) {
    for (const n of ["meridian.exe", "meridian"]) {
      const c = path.join(d, n);
      if (fs.existsSync(c) && fs.statSync(c).isFile()) return c;
    }
  }
  for (const n of ["meridian.exe", "meridian"]) {
    const c = path.join(path.dirname(process.execPath), n);
    if (fs.existsSync(c) && fs.statSync(c).isFile()) return c;
  }
  return null;
}

export function bootContext(
  workspace: string,
): [string, number, string[]] {
  const mer = findMeridian();
  if (mer) {
    try {
      const out = spawnSync(mer, ["boot", "--repo", workspace], {
        encoding: "utf8",
        timeout: 60_000,
        windowsHide: true,
      });
      if (out.status === 0) {
        const v = JSON.parse(out.stdout ?? "{}");
        return [v.context ?? "", v.tokens ?? 0, v.warnings ?? []];
      }
    } catch {}
  }
  return [
    systemSnippet(),
    0,
    ["meridian bridge not found; boot context is the contract snippet only"],
  ];
}

export function runHooks(
  cfg: store.Config,
  event: string,
  tool: string,
  payload: Record<string, any>,
  session: string,
  log: (s: string) => void = (s) => console.error(s),
): string | null {
  const ev = JSON.stringify({ event, tool, data: payload, session });
  for (const h of cfg.hooks ?? []) {
    if (h.event !== event) continue;
    const m = h.matcher ?? "";
    if (m !== "" && m !== "*" && m !== tool) continue;
    let out;
    try {
      out = spawnSync(h.command, {
        input: ev,
        shell: true,
        encoding: "utf8",
        timeout: 60_000,
        windowsHide: true,
      });
    } catch {
      continue;
    }
    if (out.status === 2) return out.stderr ?? "";
    if (out.status !== 0) {
      log(`hook '${h.command}' exited ${out.status} — continuing`);
    }
  }
  return null;
}

function cost(prices: Record<string, any>, u: Record<string, number>): number | null {
  if (!["input", "cache_read", "cache_write", "output"].some((k) => prices[k])) {
    return null;
  }
  return (
    (u.input * (prices.input ?? 0) +
      u.cache_read * (prices.cache_read ?? 0) +
      u.cache_write * (prices.cache_write ?? 0) +
      u.output * (prices.output ?? 0)) /
    1_000_000
  );
}

export type Emit = (ev: proto.Event) => void;
export type Asker = (tool: string) => Promise<boolean>;

const ZERO_USAGE = () => ({ input: 0, cache_read: 0, cache_write: 0, output: 0 });

export class Agent {
  opts: proto.RunOpts;
  home: string;
  cfg: store.Config;
  workspace: string;
  provider: Record<string, any>;
  modelAlias: string;
  mode: string;
  ctx: t.Ctx;
  session: store.Session;
  messages: store.Message[];
  bootTokens: number;
  system: string[];
  tools: Record<string, any>[];
  allowed: string[];
  cumUsd = 0;
  lastUsage = ZERO_USAGE();
  asker: Asker | null = null;
  pendingPlan: string | null = null;
  worktrees: [string, string][] = [];
  /** streamed assistant text, as it arrives (the TUI renders it live) */
  onDelta: ((text: string) => void) | null = null;
  /** diagnostics sink — stderr for headless runs, silent inside the TUI */
  log: (s: string) => void = (s) => console.error(s);
  private abort: AbortController | null = null;
  world: world.World = world.empty();
  lastModelAt: number | null = null;
  prefixHash = "";
  doom: string[] = [];
  sandboxMode = "workspace-write";
  askMode = "on-request";
  improveEnabled = true;
  forceCompact = false;
  compactFocus: string | null = null;
  staleHits = 0;
  lastL5Before: number | null = null;
  lastL5After: number | null = null;

  constructor(opts: proto.RunOpts, harness: string, cwd: string) {
    this.opts = opts;
    this.home = proto.cliHome();
    this.cfg = store.loadConfig(this.home);
    this.workspace = opts.cwd ?? cwd;
    const alias = opts.model ?? this.cfg.default_model ?? "fixture";
    const found = store.providerFor(this.cfg, alias);
    let modelId: string;
    if (found) {
      [this.provider, modelId] = found;
    } else {
      this.provider = {
        kind: "fixture",
        base_url: "",
        api_key_env: "",
        model: "fixture",
        context: 128000,
        prices: {},
      };
      modelId = "fixture";
    }
    this.modelAlias = `${alias} (${modelId})`;
    this.mode = opts.yolo ? "yolo" : opts.mode ?? "agent";
    this.ctx = new t.Ctx(this.workspace);
    this.ctx.mcpServers = this.cfg.mcp ?? {};
    this.ctx.largeReadLines = this.cfg.bridge.large_read_lines ?? 200;
    this.ctx.noSandbox = opts.no_sandbox;
    this.ctx.escalations = [...opts.allow_escalation];
    this.ctx.netAllow = opts.allow_escalation.includes("network");
    if (opts.resume) {
      this.session = store.Session.open(this.home, opts.resume);
      this.session.harness = harness;
      this.messages = this.session.replay();
    } else {
      this.session = store.Session.create(this.home, harness);
      this.messages = [];
    }
    const [boot, bootTokens] = bootContext(this.workspace);
    this.bootTokens = bootTokens;
    this.system = boot.includes("You are Cosmos")
      ? [boot]
      : [systemSnippet(), boot];
    // AGENTS.md / CLAUDE.md / .zcli/instructions.md from the repo
    const project = store.projectInstructions(this.workspace);
    if (project) this.system.push(project);
    this.tools = t.toolSpecs();
    this.allowed = store.allowedHosts(this.cfg);
    this.ctx.home = this.home;
    this.ctx.sessionId = this.session.id;
    {
      const paths = new Map<string, string>();
      for (const m of this.messages) {
        for (const b of m.content as any[]) {
          if (b.type === "tool_use" && b.name === "read" && b.input?.path) {
            paths.set(b.id, b.input.path);
          }
          if (b.type === "tool_result" && !b.is_error) {
            const sha = String(b.content ?? "").match(/file_sha256=([0-9a-f]{64})/)?.[1];
            const rel = paths.get(b.tool_use_id);
            if (sha && rel) {
              try {
                this.ctx.readState.set(t.resolve(this.ctx, rel), sha);
              } catch {}
            }
          }
        }
      }
    }
    this.sandboxMode = opts.sandbox ?? this.cfg.sandbox?.mode ?? "workspace-write";
    this.askMode = opts.ask ?? this.cfg.sandbox?.ask ?? "on-request";
    this.ctx.sandboxMode = this.sandboxMode;
    this.world = world.load(this.session.id, this.home);
    this.prefixHash = compact.prefixHash(this.system, t.TOOLS_JSON);
    this.improveEnabled =
      opts.self_improve ?? (opts.output === "text" && (this.cfg.self_improve?.enabled ?? true));
  }

  /** Stop the provider stream and any running tool right now. */
  cancel(): void {
    this.abort?.abort();
  }

  get cancelled(): boolean {
    return !!this.abort?.signal.aborted;
  }

  /** Output budget per turn: the model's own limit when known, capped. */
  maxTokens(): number {
    const out = Number(this.provider.output) || 0;
    return out > 0 ? Math.max(4096, Math.min(out, 32_000)) : 8192;
  }

  cleanup(): void {
    this.abort?.abort();
    // task{worktree:true} checkouts — removed only if clean (§10).
    for (const [wt, branch] of this.worktrees) {
      try {
        const clean = spawnSync(
          "git",
          ["-C", wt, "status", "--porcelain"],
          { encoding: "utf8", windowsHide: true },
        );
        if (clean.status === 0 && !(clean.stdout ?? "").trim()) {
          spawnSync(
            "git",
            ["-C", this.ctx.workspace, "worktree", "remove", wt],
            { encoding: "utf8", windowsHide: true },
          );
          spawnSync(
            "git",
            ["-C", this.ctx.workspace, "branch", "-D", branch],
            { encoding: "utf8", windowsHide: true },
          );
        }
      } catch {}
    }
    this.worktrees = [];
  }

  permitted(name: string): string {
    if (this.mode === "yolo") return "allow";
    if (this.mode === "plan" && !t.PLAN_TOOLS.has(name)) return "deny";
    if (this.mode === "accept-edits" && (name === "write" || name === "edit")) return "allow";
    if (
      this.opts.allow.includes(name) ||
      (this.cfg.permissions.allow ?? []).includes(name)
    ) {
      return "allow";
    }
    if ((this.cfg.permissions.ask ?? []).includes(name)) {
      if (this.askMode === "never") return "deny";
      return "ask";
    }
    return "deny";
  }

  compactNow(focus: string | undefined, emit: Emit): void {
    this.forceCompact = true;
    this.compactFocus = focus ?? null;
    this.maybeCompact(emit);
    this.forceCompact = false;
    this.compactFocus = null;
  }

  undoLast(emit: Emit): string {
    try {
      const p = ckpt.restoreLast(this.session.id, this.home);
      emit(this.session.append("checkpoint", { op: "restore", path: p }));
      return `restored ${p}`;
    } catch (e: any) {
      return `undo failed: ${e?.message ?? e}`;
    }
  }

  skillify(name: string, emit: Emit): string {
    if (!this.improveEnabled && !this.opts.yolo) return "self_improve disabled";
    try {
      const dest = improve.skillify(name, this.world, false, this.home, !!this.opts.allow_self_mutate);
      emit(this.session.append("self_improve", improve.eventData("skillify", { kind: "skill", name })));
      return `wrote ${dest}`;
    } catch (e: any) {
      return String(e?.message ?? e);
    }
  }

  contextReport(): string {
    const tokens = compact.estimateTokens(this.messages, this.bootTokens);
    const budget = this.provider.context ?? 128000;
    const pct = budget ? Math.floor((tokens * 100) / budget) : 0;
    const warm = compact.cacheWarm(this.lastModelAt, this.prefixHash, this.prefixHash, Date.now());
    return JSON.stringify({ tokens_in: tokens, tokens_budget: budget, percent: pct, cache_warm: warm });
  }

  sendUser(text: string, emit: Emit): void {
    emit(this.session.append("user", { text }));
    world.setGoalIfEmpty(this.world, text);
    world.save(this.session.id, this.world, this.home);
    this.messages.push({
      role: "user",
      content: [{ type: "text", text }],
    });
  }

  approvePlan(emit: Emit): void {
    if (this.pendingPlan) {
      const plan = this.pendingPlan;
      this.pendingPlan = null;
      this.mode = "agent";
      emit(this.session.append("mode", { from: "plan", to: "agent" }));
      this.messages.push({
        role: "user",
        content: [{ type: "text", text: `[approved plan]\n${plan}` }],
      });
    }
  }

  async turn(emit: Emit): Promise<boolean> {
    // One provider turn. True when no tool calls were made.
    await this.maybeCompact(emit);
    let text = "";
    const calls: Record<string, any>[] = [];
    const usage = ZERO_USAGE();
    const err: string[] = [];

    const sink = (c: Record<string, any>) => {
      if (c.type === "text") {
        text += c.text;
        this.onDelta?.(c.text);
      } else if (c.type === "tool_use") calls.push(c);
      else if (c.type === "done") {
        for (const k of Object.keys(usage) as (keyof typeof usage)[]) {
          usage[k] += c.usage?.[k] ?? 0;
        }
      } else if (c.type === "err") err.push(c.text);
    };

    this.abort = new AbortController();
    try {
      await providers.complete(
        this.provider,
        {
          system: this.system,
          tools: this.tools,
          messages: compact.layer4(this.messages),
          session_id: this.session.id,
          max_tokens: this.maxTokens(),
        },
        sink,
        this.allowed,
        this.abort.signal,
      );
    } catch (e) {
      if (this.abort.signal.aborted) {
        // keep what streamed so far — the transcript and the next turn see it
        if (text) {
          emit(this.session.append("assistant", { text, tool_calls: [], interrupted: true }));
          this.messages.push({ role: "assistant", content: [{ type: "text", text }] });
        }
        throw new Error("interrupted");
      }
      throw e;
    }
    if (err.length) throw new Error(`provider stream error: ${err[0]}`);
    this.lastUsage = usage;
    const c = cost(this.provider.prices ?? {}, usage);
    this.cumUsd += c ?? 0;
    emit(
      this.session.append("assistant", {
        text,
        tool_calls: calls.map((c2) => ({
          id: c2.id,
          name: c2.name,
          input: c2.input,
        })),
      }),
    );
    emit(
      this.session.append("usage", {
        input: usage.input,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        output: usage.output,
        usd: c,
        cum_usd: this.cumUsd,
      }),
    );
    this.log(
      `turn · in ${usage.input} · cache_r ${usage.cache_read} ` +
        `· cache_w ${usage.cache_write} · out ${usage.output} · ` +
        `${c != null ? `$${c.toFixed(4)}` : "$?"} · ` +
        `Σ $${this.cumUsd.toFixed(4)} · ctx ${JSON.parse(this.contextReport()).percent}%`,
    );
    const blocks: Record<string, any>[] = text ? [{ type: "text", text }] : [];
    for (const c2 of calls) {
      blocks.push({ type: "tool_use", id: c2.id, name: c2.name, input: c2.input });
    }
    this.messages.push({ role: "assistant", content: blocks });
    this.lastModelAt = Date.now();
    return calls.length === 0;
  }

  async execTool(
    tid: string,
    name: string,
    inp: Record<string, any>,
    emit: Emit,
  ): Promise<[string, boolean]> {
    const repaired = repair.repair(name, inp);
    for (const r of repaired.repairs) {
      emit(this.session.append("tool_repair", { from: r.from, to: r.to, reason: r.reason }));
    }
    if (repaired.reject) {
      emit(this.session.append("tool_call", { id: tid, name, input: inp }));
      emit(this.session.append("tool_result", { tool_use_id: tid, name, content: repaired.reject, is_error: true }));
      this.messages.push({
        role: "user",
        content: [{ type: "tool_result", tool_use_id: tid, content: repaired.reject, is_error: true }],
      });
      return [repaired.reject, true];
    }
    name = repaired.name;
    inp = repaired.input;
    const h = compact.callHash(name, inp);
    this.doom.push(h);
    if (this.doom.length > 3) this.doom.shift();
    if (this.doom.length === 3 && this.doom[0] === this.doom[1] && this.doom[1] === this.doom[2]) {
      const msg = "doom_loop: same tool+args fired 3 times with no file change — stop and ask";
      emit(this.session.append("doom_loop", { tool: name, input: inp }));
      this.messages.push({
        role: "user",
        content: [{ type: "tool_result", tool_use_id: tid, content: msg, is_error: true }],
      });
      return [msg, true];
    }
    this.ctx.currentToolId = tid;
    emit(this.session.append("tool_call", { id: tid, name, input: inp }));
    const blocked = runHooks(this.cfg, "pre_tool", name, inp, this.session.id, this.log);
    let text: string;
    let isError: boolean;
    if (blocked !== null) {
      [text, isError] = [blocked, true];
    } else {
      const perm = this.permitted(name);
      if (perm === "deny") {
        [text, isError] = [
          `permission denied: ${name} is not allowed in ${this.mode} mode`,
          true,
        ];
      } else if (perm === "ask") {
        this.session.append("permission_request", { tool: name, input: inp });
        const granted = this.asker ? await this.asker(name) : false;
        emit(
          this.session.append("permission_decision", {
            tool: name,
            input: inp,
            granted,
          }),
        );
        if (granted) {
          [text, isError] = await this.dispatch(name, inp, emit);
        } else {
          [text, isError] = [
            `permission denied: ${name} requires operator approval`,
            true,
          ];
        }
      } else {
        [text, isError] = await this.dispatch(name, inp, emit);
      }
    }
    emit(
      this.session.append("tool_result", {
        tool_use_id: tid,
        name,
        content: text,
        is_error: isError,
      }),
    );
    runHooks(this.cfg, "post_tool", name, { input: inp, is_error: isError }, this.session.id, this.log);
    this.messages.push({
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: tid,
          content: text,
          is_error: isError,
        },
      ],
    });
    return [text, isError];
  }

  private async dispatch(
    name: string,
    inp: Record<string, any>,
    emit: Emit,
  ): Promise<[string, boolean]> {
    if (name === "submit_plan") {
      const plan = inp.plan ?? "";
      this.pendingPlan = plan;
      emit(this.session.append("plan", { plan }));
      return [`plan submitted (${plan.length} chars)`, false];
    }
    if (name === "request_escalation") {
      const cap = inp.capability ?? "";
      const granted = this.ctx.escalations.includes(cap);
      emit(
        this.session.append("permission_decision", {
          tool: "request_escalation",
          capability: cap,
          granted,
        }),
      );
      if (granted) {
        if (cap === "network") this.ctx.netAllow = true;
        return [`escalation granted: ${cap}`, false];
      }
      return [
        `escalation denied: ${cap} — rerun with --allow-escalation ${cap}`,
        true,
      ];
    }
    if (name === "task") {
      const write = !!inp.write;
      const worktree = inp.worktree ?? write;
      return [await this.subagent(inp.prompt ?? "", !!worktree, emit), false];
    }
    if (name === "context_remaining") return [this.contextReport(), false];
    if (name === "new_context") {
      this.compactNow(undefined, emit);
      return [`reseeding from world.md\n${world.render(this.world)}`, false];
    }
    if (!this.abort || this.abort.signal.aborted) this.abort = new AbortController();
    const out = await t.runAsync(this.ctx, name, inp, this.abort.signal);
    const err = out.startsWith("ERROR") || out.includes("STALE");
    if (err && out.includes("STALE")) {
      this.staleHits += 1;
      if (this.staleHits >= 2 && this.improveEnabled) {
        emit(this.session.append("self_improve", improve.eventData("correction", { text: improve.correctionRow(name, "re-read then edit") })));
      }
    }
    if (!err && (name === "write" || name === "edit") && inp.path) {
      try {
        const abs = t.resolve(this.ctx, inp.path);
        const bytes = fs.readFileSync(abs);
        world.touchFile(this.world, inp.path, bytes);
        world.save(this.session.id, this.world, this.home);
      } catch {}
      const diag = verify.afterWrite(this.workspace, inp.path);
      if (diag) emit(this.session.append("verify", { path: inp.path, text: diag }));
    }
    return [out, err];
  }

  private addWorktree(childId: string): string {
    const wt = path.join(this.home, "worktrees", this.session.id, childId);
    const branch = `cosmos-task-${childId}`;
    const p = spawnSync(
      "git",
      ["-C", this.ctx.workspace, "worktree", "add", wt, "-b", branch],
      { encoding: "utf8", windowsHide: true },
    );
    if (p.status !== 0) throw new Error((p.stderr ?? "").trim());
    this.worktrees.push([wt, branch]);
    return wt;
  }

  private childCtx(workspace: string): t.Ctx {
    const c = new t.Ctx(workspace);
    c.escalations = [...this.ctx.escalations];
    c.netAllow = this.ctx.netAllow;
    c.noSandbox = this.ctx.noSandbox;
    c.largeReadLines = this.ctx.largeReadLines;
    c.mcpServers = { ...this.ctx.mcpServers };
    c.home = this.ctx.home;
    c.sessionId = this.ctx.sessionId;
    c.sandboxMode = this.ctx.sandboxMode;
    return c;
  }

  private async subagent(
    prompt: string,
    worktree: boolean,
    emit: Emit,
  ): Promise<string> {
    const child = store.Session.create(this.home, this.session.harness);
    let wtNote = "";
    if (worktree) {
      try {
        this.addWorktree(child.id);
        wtNote = `\n[worktree branch: cosmos-task-${child.id}]`;
      } catch (e: any) {
        wtNote = `\n[worktree unavailable: ${e?.message ?? e}]`;
      }
    }
    const childCtx = this.worktrees.length
      ? this.childCtx(this.worktrees[this.worktrees.length - 1][0])
      : null;
    emit(
      this.session.append("subagent_start", {
        child_session: child.id,
        worktree: this.worktrees.length
          ? this.worktrees[this.worktrees.length - 1][0]
          : null,
        prompt: prompt.slice(0, 200),
      }),
    );
    child.append("session_start", {
      model: this.modelAlias,
      mode: "agent",
      sandbox: "appcontainer",
      boot_tokens: this.bootTokens,
      parent_session: this.session.id,
    });
    const msgs: store.Message[] = [
      { role: "user", content: [{ type: "text", text: prompt }] },
    ];
    let result = "subagent turn cap reached";
    for (let i = 0; i < 8; i++) {
      let text = "";
      const calls: Record<string, any>[] = [];
      const sink = (c: Record<string, any>) => {
        if (c.type === "text") text += c.text;
        else if (c.type === "tool_use") calls.push(c);
      };
      try {
        await providers.complete(
          this.provider,
          {
            system: this.system,
            tools: this.tools,
            messages: msgs,
            session_id: child.id,
            max_tokens: Math.min(this.maxTokens(), 8192),
          },
          sink,
          this.allowed,
          this.abort?.signal,
        );
      } catch (e: any) {
        result = this.cancelled ? "subagent interrupted" : `subagent provider error: ${e?.message ?? e}`;
        break;
      }
      const blocks: Record<string, any>[] = text ? [{ type: "text", text }] : [];
      for (const c of calls) {
        blocks.push({ type: "tool_use", id: c.id, name: c.name, input: c.input });
      }
      msgs.push({ role: "assistant", content: blocks });
      if (!calls.length) {
        result = text.slice(0, 4000);
        break;
      }
      for (const c of calls) {
        const tctx = childCtx ?? this.ctx;
        const out = t.PLAN_TOOLS.has(c.name)
          ? await t.runAsync(tctx, c.name, c.input, this.abort?.signal)
          : `subagent tool denied: ${c.name}`;
        msgs.push({
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: c.id,
              content: out,
              is_error: false,
            },
          ],
        });
      }
    }
    emit(this.session.append("subagent_end", { child_session: child.id }));
    return result + wtNote;
  }

  maybeCompact(emit: Emit): void {
    const prefixNow = compact.prefixHash(this.system, t.TOOLS_JSON);
    const warm = compact.cacheWarm(this.lastModelAt, prefixNow, this.prefixHash, Date.now());
    runHooks(this.cfg, "pre_compact", "", {}, this.session.id, this.log);
    const outcome = compact.compact(this.messages, this.session.id, {
      home: this.home,
      largeReadLines: this.ctx.largeReadLines,
      context: this.provider.context ?? 128000,
      boot: this.bootTokens,
      warm,
      force: this.forceCompact,
      focus: this.compactFocus ?? undefined,
      lastL5Before: this.lastL5Before,
      lastL5After: this.lastL5After,
    });
    if (outcome.world) {
      world.applyCompactMd(this.world, outcome.world);
      world.save(this.session.id, this.world, this.home);
      this.lastL5Before = outcome.before;
      this.lastL5After = outcome.after;
    }
    for (const ev of outcome.events) {
      emit(this.session.append(ev.kind, ev.data));
    }
    this.prefixHash = prefixNow;
  }

  async execToolsBatch(
    calls: [string, string, any][],
    emit: Emit,
  ): Promise<[string, string, boolean][]> {
    const ro = calls.filter((c) => t.READONLY.has(c[1]));
    const wr = calls.filter((c) => !t.READONLY.has(c[1]));
    const out: [string, string, boolean][] = [];
    if (ro.length > 1) {
      const slice = ro.slice(0, 8);
      const rest = ro.slice(8);
      const results = await Promise.all(
        slice.map(async ([tid, name, inp]) => {
          const ctx = this.childCtx(this.ctx.workspace);
          ctx.currentToolId = tid;
          ctx.readState = new Map(this.ctx.readState);
          const text = await t.runAsync(ctx, name, inp);
          return { tid, name, inp, text, state: ctx.readState };
        }),
      );
      for (const r of results) {
        for (const [k, v] of r.state) this.ctx.readState.set(k, v);
        emit(this.session.append("tool_call", { id: r.tid, name: r.name, input: r.inp }));
        const err = r.text.startsWith("ERROR");
        emit(this.session.append("tool_result", { tool_use_id: r.tid, name: r.name, content: r.text, is_error: err }));
        this.messages.push({
          role: "user",
          content: [{ type: "tool_result", tool_use_id: r.tid, content: r.text, is_error: err }],
        });
        out.push([r.name, r.text, err]);
      }
      for (const c of rest) {
        const [text, err] = await this.execTool(c[0], c[1], c[2], emit);
        out.push([c[1], text, err]);
      }
    } else {
      for (const c of ro) {
        const [text, err] = await this.execTool(c[0], c[1], c[2], emit);
        out.push([c[1], text, err]);
      }
    }
    for (const c of wr) {
      const [text, err] = await this.execTool(c[0], c[1], c[2], emit);
      out.push([c[1], text, err]);
    }
    return out;
  }
}

export function emitter(mode: string): Emit {
  return (ev) => {
    if (mode === "json-stream") {
      process.stdout.write(proto.toLine(ev) + "\n");
    } else if (mode === "json") {
      process.stdout.write(JSON.stringify(ev, null, 2) + "\n");
    } else {
      if (ev.kind === "assistant" && ev.data?.text) {
        console.log(ev.data.text);
      } else if (ev.kind === "tool_result") {
        console.error(
          `  [${ev.data?.name}] ${String(ev.data?.content ?? "").slice(0, 200)}`,
        );
      }
    }
  };
}

export async function runHeadless(
  opts: proto.RunOpts,
  harness: string,
  cwd: string,
): Promise<number> {
  let prompt = opts.prompt;
  if (prompt === "-") {
    prompt = await new Promise<string>((res) => {
      let d = "";
      process.stdin.on("data", (c) => (d += c));
      process.stdin.on("end", () => res(d));
      process.stdin.resume();
    });
  }
  if (prompt === null || prompt === undefined) {
    throw new Error("run_headless needs a prompt");
  }
  const agent = new Agent(opts, harness, cwd);
  try {
    const emit = emitter(opts.output);
    emit(
      agent.session.append("session_start", {
        model: agent.modelAlias,
        mode: agent.mode,
        sandbox: agent.ctx.noSandbox ? "off" : "appcontainer",
        boot_tokens: agent.bootTokens,
        resume: opts.resume,
      }),
    );
    runHooks(agent.cfg, "session_start", "", {}, agent.session.id);
    if (prompt.startsWith("/compact")) {
      let focus = prompt.slice("/compact".length).trim();
      if (focus.startsWith("focus:")) focus = focus.slice(6).trim();
      else if (focus.startsWith("focus")) focus = focus.slice(5).trim();
      agent.compactNow(focus || undefined, emit);
      emit(agent.session.append("session_end", { reason: "compact" }));
      return proto.EXIT_DONE;
    }
    if (prompt.trim() === "/undo") {
      const msg = agent.undoLast(emit);
      if (opts.output === "text") console.error(msg);
      emit(agent.session.append("session_end", { reason: "undo" }));
      return proto.EXIT_DONE;
    }
    if (prompt.startsWith("/skillify")) {
      const name = prompt.slice("/skillify".length).trim() || "session";
      const msg = agent.skillify(name, emit);
      if (opts.output === "text") console.error(msg);
      emit(agent.session.append("session_end", { reason: "skillify" }));
      return proto.EXIT_DONE;
    }
    agent.sendUser(prompt, emit);
    runHooks(agent.cfg, "user_prompt", "", { text: prompt }, agent.session.id);

    for (let i = 0; i < (opts.max_turns ?? 16); i++) {
      if (opts.max_usd !== null && agent.cumUsd >= opts.max_usd) {
        return proto.EXIT_BUDGET;
      }
      const done = await agent.turn(emit);
      if (done) {
        if (opts.autonomous_gate) {
          const gate = opts.autonomous_gate;
          const out = spawnSync(
            process.platform === "win32" ? "cmd" : "sh",
            process.platform === "win32" ? ["/C", gate] : ["-lc", gate],
            { cwd: agent.workspace, encoding: "utf8", windowsHide: true },
          );
          if (out.status !== 0) {
            const body = `${out.stdout ?? ""}${out.stderr ?? ""}`;
            agent.messages.push({
              role: "user",
              content: [{ type: "text", text: `[autonomous-gate failed]\n${body}` }],
            });
            continue;
          }
        }
        if (agent.improveEnabled) {
          try {
            const draft = `draft-${agent.session.id.slice(0, 8)}`;
            improve.skillify(draft, agent.world, true, agent.home, !!opts.allow_self_mutate);
            emit(agent.session.append("self_improve", improve.eventData("draft_skill", { name: draft, draft: true })));
          } catch {}
        }
        emit(agent.session.append("session_end", { reason: "done" }));
        return proto.EXIT_DONE;
      }
      const calls = agent.messages[agent.messages.length - 1].content
        .filter((b) => b.type === "tool_use")
        .map((b) => [b.id, b.name, b.input] as [string, string, any]);
      let denied = false;
      const results = await agent.execToolsBatch(calls, emit);
      for (const [i2, [name, text, isError]] of results.entries()) {
        if (name === "submit_plan" && !isError) {
          if (opts.approve_plan) {
            agent.approvePlan(emit);
          } else {
            emit(agent.session.append("session_end", { reason: "plan_submitted" }));
            return proto.EXIT_DONE;
          }
        }
        if (isError && text.startsWith("permission denied")) denied = true;
        void i2;
      }
      if (denied) {
        emit(agent.session.append("session_end", { reason: "permission_denied" }));
        return proto.EXIT_DENIED;
      }
    }
    emit(agent.session.append("session_end", { reason: "max_turns" }));
    return proto.EXIT_FAILED;
  } finally {
    agent.cleanup();
  }
}
