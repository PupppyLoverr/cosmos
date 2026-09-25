// ZdevDriver — in-process backend with the AcpClient surface. The TUI talks
// to the Agent directly (no stdio hop); same semantics as `zdev acp`:
//   call(method, params) → {result} | {error}
//   onUpdate(sid, update) fires for session/update-mapped events, plus
//     `agent_message_chunk` deltas while the reply streams
//   server→client session/request_permission → onRequest + reply(id, …)
//   notify("session/cancel") → aborts the provider stream / running tool
import fs from "node:fs";
import path from "node:path";

import { Agent } from "../agent";
import { updateFor, type Json } from "../acp";
import * as catalog from "../catalog";
import * as creds from "../creds";
import * as live from "../live";
import * as proto from "../proto";
import { lastRateLimits } from "../providers";
import * as store from "../store";

export type ServerRequest = { id: number; method: string; params: any };

/** Bump when the onboarding flow changes — existing installs re-run once. */
export const ONBOARDING_VERSION = "3";

export class ZdevDriver {
  private opts: proto.RunOpts;
  private cwd: string;
  agent: Agent | null = null;
  private cancelled = false;
  private askerCtx: Json = {};
  private reqSeq = 1000;
  private clientReplies = new Map<number, (result: any) => void>();
  /** provider diagnostics (turn usage, hook failures) — never printed in the TUI */
  stderrTail: string[] = [];

  onUpdate: (sessionId: string, update: any) => void = () => {};
  onRequest: (req: ServerRequest) => void = () => {};

  constructor(opts: proto.RunOpts, cwd: string) {
    this.opts = opts;
    this.cwd = cwd;
  }

  get home(): string {
    return proto.cliHome();
  }

  get sessionId(): string {
    return this.agent?.session.id ?? "";
  }

  // ── onboarding / identity ──────────────────────────────────────────────

  static onboardingDone(home = proto.cliHome()): boolean {
    try {
      return fs.readFileSync(path.join(home, ".zdev-onboarded"), "utf8").trim() === ONBOARDING_VERSION;
    } catch {
      return false;
    }
  }

  static markOnboarded(home = proto.cliHome()): void {
    fs.mkdirSync(home, { recursive: true });
    fs.writeFileSync(path.join(home, ".zdev-onboarded"), ONBOARDING_VERSION + "\n", "utf8");
  }

  /** `user_name = "…"` in config.toml — drives "Welcome back <name>". */
  userName(): string {
    const v = store.loadConfig(this.home).user_name;
    return typeof v === "string" ? v : "";
  }

  sessions(): Record<string, any>[] {
    return store.Session.list(this.home)
      .slice()
      .reverse()
      .filter((s) => s.id !== this.sessionId && s.preview);
  }

  // ── keys, sign-in, live models (/connect, /login, /logout, /model) ───────

  /** Catalog providers that take an API key or sign-in, with status. */
  providers(): { def: catalog.ProviderDef; ready: boolean; detail: string }[] {
    return catalog
      .providers()
      .filter((p) => catalog.isSubscription(p) || p.env.length)
      .map((p) => {
        if (catalog.isSubscription(p)) {
          const t = creds.chatgpt();
          return { def: p, ready: !!t, detail: t ? `signed in${t.email ? ` as ${t.email}` : ""}` : "browser sign-in" };
        }
        const k = creds.keyFor(p.id, p.env);
        return { def: p, ready: !!k, detail: k ? `${creds.mask(k[0])} · ${k[1]}` : p.env[0] };
      });
  }

  storeKey(providerId: string, key: string): creds.Source {
    return creds.storeKey(providerId, key);
  }

  deleteKey(providerId: string): void {
    creds.deleteKey(providerId);
  }

  async fetchModels(providerId: string, signal?: AbortSignal): Promise<catalog.ModelDef[]> {
    const def = catalog.provider(providerId);
    if (!def) throw new Error(`unknown provider ${providerId}`);
    const models = await live.fetchModels(def, signal);
    live.store(providerId, models);
    return models;
  }

  async login(onUrl: (url: string) => void, signal?: AbortSignal) {
    const { login } = await import("../oauth");
    return login(onUrl, signal);
  }

  async logout(providerId: string): Promise<string> {
    if (providerId === "chatgpt") {
      const { logout } = await import("../oauth");
      logout();
      return "Signed out of ChatGPT";
    }
    creds.deleteKey(providerId);
    return `Removed the stored ${providerId} key (environment variables are untouched)`;
  }

  rateLimits() {
    return lastRateLimits();
  }

  // ── AcpClient-shaped surface ───────────────────────────────────────────
  async call(method: string, params: any): Promise<any> {
    try {
      return { result: await this.dispatch(method, params ?? {}) };
    } catch (e: any) {
      return { error: { code: -32603, message: String(e?.message ?? e) } };
    }
  }

  notify(method: string, _params: any) {
    if (method === "session/cancel") {
      this.cancelled = true;
      this.agent?.cancel();
      // a pending permission prompt counts as "deny" so the turn can unwind
      for (const [id, r] of this.clientReplies) {
        this.clientReplies.delete(id);
        r({ outcome: { outcome: "cancelled" } });
      }
    }
  }

  reply(id: number, result: any) {
    const r = this.clientReplies.get(id);
    if (r) {
      this.clientReplies.delete(id);
      r(result);
    }
  }

  kill() {
    this.agent?.cleanup();
    this.agent = null;
  }

  /** Map session events to UI updates. Assistant text already streamed as
   * deltas, so the final `assistant` event is not re-sent. */
  private emit = (sid: string) => (ev: proto.Event) => {
    if (ev.kind === "assistant") return;
    // you just approved it — only a denial is worth a line
    if (ev.kind === "permission_decision" && ev.data?.granted) return;
    const n = updateFor(sid, ev);
    if (n) this.onUpdate(sid, n.params.update);
  };

  private attach(a: Agent) {
    this.agent = a;
    this.cancelled = false;
    this.askerCtx = {};
    a.asker = this.makeAsker(a);
    a.log = (s) => {
      this.stderrTail.push(s);
      if (this.stderrTail.length > 50) this.stderrTail.shift();
    };
    a.onDelta = (text) =>
      this.onUpdate(a.session.id, { sessionUpdate: "agent_message_chunk", content: { type: "text", text } });
  }

  private async dispatch(method: string, params: Json): Promise<Json> {
    switch (method) {
      case "initialize":
        return {
          protocolVersion: 1,
          agentInfo: { name: "zdev", version: "0.1.0" },
          agentCapabilities: { loadSession: false, promptCapabilities: { embeddedContext: true } },
        };
      case "session/new": {
        const o = new proto.RunOpts();
        Object.assign(o, this.opts);
        o.allow = [...this.opts.allow];
        o.cwd = params.cwd ?? this.opts.cwd ?? this.cwd;
        if (["plan", "agent", "yolo"].includes(params.mode)) {
          o.mode = params.mode;
          o.yolo = params.mode === "yolo";
        }
        if (params.model) o.model = params.model;
        o.resume = params.resume ?? null;
        this.agent?.cleanup();
        const a = new Agent(o, "zdev", o.cwd ?? this.cwd);
        this.attach(a);
        return {
          sessionId: a.session.id,
          model: a.modelAlias,
          mode: a.mode,
          context: a.provider.context ?? 0,
          history: o.resume ? a.messages.length : 0,
        };
      }
      case "session/prompt": {
        const a = this.mustAgent();
        this.cancelled = false;
        const prompt = (params.prompt ?? []).map((b: Json) => b.text ?? "").join("\n");
        const emit = this.emit(a.session.id);
        a.sendUser(prompt, emit);
        return { stopReason: await this.runTurns(a, emit) };
      }
      case "session/approve": {
        const a = this.mustAgent();
        this.cancelled = false;
        const emit = this.emit(a.session.id);
        a.approvePlan(emit);
        return { stopReason: await this.runTurns(a, emit) };
      }
      case "session/set_mode": {
        const a = this.mustAgent();
        const mode = params.mode ?? "";
        if (!["plan", "agent", "yolo"].includes(mode)) throw new Error(`bad mode ${JSON.stringify(mode)} (plan|agent|yolo)`);
        const old = a.mode;
        a.mode = mode;
        a.opts.yolo = mode === "yolo";
        a.session.append("mode", { from: old, to: mode });
        return { mode };
      }
      case "session/set_model": {
        const a = this.mustAgent();
        const alias = String(params.model ?? "");
        // reload config — onboarding or /connect may have changed it since boot
        a.cfg = store.loadConfig(this.home);
        const found = store.providerFor(a.cfg, alias);
        if (!found) throw new Error(`unknown model ${JSON.stringify(alias)} — try provider/model, e.g. anthropic/claude-opus-5-5`);
        const [provider, modelId] = found;
        if (params.effort) provider.effort = params.effort;
        a.provider = provider;
        a.allowed = store.allowedHosts(a.cfg);
        a.modelAlias = `${alias} (${modelId})`;
        a.session.append("model_switch", { alias, id: modelId });
        if (params.remember) store.setDefaultModel(alias, this.home);
        return { model: a.modelAlias, context: provider.context ?? 0 };
      }
      case "session/list_models": {
        const cfg = store.loadConfig(this.home);
        return { models: store.availableModels(cfg), default: cfg.default_model };
      }
      default:
        throw new Error("method not found");
    }
  }

  private mustAgent(): Agent {
    if (!this.agent) throw new Error("no session (call session/new first)");
    return this.agent;
  }

  /** Server→client request; resolves with the client's `result` payload. */
  private requestFromClient(method: string, params: Json): Promise<any> {
    const id = this.reqSeq++;
    return new Promise((res) => {
      this.clientReplies.set(id, res);
      this.onRequest({ id, method, params });
    });
  }

  private makeAsker(a: Agent) {
    return async (tool: string): Promise<boolean> => {
      if (this.cancelled) return false;
      const result = await this.requestFromClient("session/request_permission", {
        sessionId: a.session.id,
        toolCallId: this.askerCtx.tid ?? tool,
        tool,
        input: this.askerCtx.input ?? {},
        options: [
          { id: "allow", name: "Yes" },
          { id: "allow_always", name: `Yes, and don't ask again for ${tool} this session` },
          { id: "deny", name: "No, tell zdev what to do differently" },
        ],
      });
      const outcome = result?.outcome ?? {};
      const opt = outcome.optionId ?? outcome.outcome ?? "deny";
      if (opt === "allow_always" || opt === "always") {
        if (!a.opts.allow.includes(tool)) a.opts.allow.push(tool);
        return true;
      }
      return opt === "allow" || opt === "yes";
    };
  }

  private async runTurns(a: Agent, emit: (ev: proto.Event) => void): Promise<string> {
    for (let i = 0; i < (a.opts.max_turns ?? 40); i++) {
      if (this.cancelled) return "cancelled";
      let done: boolean;
      try {
        done = await a.turn(emit);
      } catch (e: any) {
        if (this.cancelled || String(e?.message) === "interrupted") return "cancelled";
        const msg = String(e?.message ?? e);
        this.stderrTail.push(msg);
        this.onUpdate(a.session.id, { sessionUpdate: "error", text: msg });
        return "refusal";
      }
      if (done) return "end_turn";
      const calls = (a.messages[a.messages.length - 1].content ?? []).filter((b) => b.type === "tool_use");
      if (!calls.length) return "end_turn";
      for (const c of calls) {
        if (this.cancelled) {
          // answer every call so the transcript stays valid for the next turn
          a.messages.push({
            role: "user",
            content: [{ type: "tool_result", tool_use_id: c.id, content: "interrupted by the operator", is_error: true }],
          });
          continue;
        }
        this.askerCtx.tid = c.id;
        this.askerCtx.input = c.input;
        await a.execTool(c.id, c.name, c.input, emit);
        if (c.name === "submit_plan") return "end_turn";
      }
      if (this.cancelled) return "cancelled";
    }
    return "max_turns";
  }
}
