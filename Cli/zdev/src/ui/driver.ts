// ZdevDriver — in-process backend with the AcpClient surface. The TUI talks
// to the Agent directly (no stdio hop); same semantics as `zdev acp`:
//   call(method, params) → {result} | {error}
//   onUpdate(sid, update) fires for session/update-mapped events
//   server→client session/request_permission → onRequest + reply(id, …)
//   notify("session/cancel") → per-session cancel flag
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { Agent } from "../agent";
import { updateFor, type Json } from "../acp";
import * as proto from "../proto";
import * as store from "../store";

export type ServerRequest = { id: number; method: string; params: any };

/** Bump when the onboarding flow changes — existing installs re-run once. */
export const ONBOARDING_VERSION = "2";

export class ZdevDriver {
  private opts: proto.RunOpts;
  private cwd: string;
  private agent: Agent | null = null;
  private cancelFlag = { set: false };
  private askerCtx: Json = {};
  private reqSeq = 1000;
  private clientReplies = new Map<number, (result: any) => void>();
  stderrTail: string[] = [];

  onUpdate: (sessionId: string, update: any) => void = () => {};
  onRequest: (req: ServerRequest) => void = () => {};
  onExit: (code: number | null) => void = () => {};

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

  // ── onboarding helpers ─────────────────────────────────────────────────
  providersInfo(): {
    name: string;
    base_url: string;
    api_key_env: string;
    keySet: boolean;
    kind: string;
  }[] {
    const cfg = store.loadConfig(this.home);
    return Object.entries(cfg.providers).map(([name, p]) => ({
      name,
      base_url: p.base_url ?? "",
      api_key_env: p.api_key_env ?? "",
      keySet: !!(p.api_key_env && process.env[p.api_key_env]),
      kind: p.kind ?? "fixture",
    }));
  }

  /** Write default_model = "<alias>" into config.toml, preserving the rest. */
  persistDefaultModel(alias: string): void {
    const p = path.join(this.home, "config.toml");
    let text = "";
    try {
      text = fs.readFileSync(p, "utf8");
    } catch {}
    const line = `default_model = "${alias}"`;
    if (/^default_model\s*=/m.test(text)) {
      text = text.replace(/^default_model\s*=.*$/m, line);
    } else {
      text = line + "\n" + text;
    }
    fs.mkdirSync(this.home, { recursive: true });
    fs.writeFileSync(p, text, "utf8");
  }

  onboardingDone(): boolean {
    try {
      return (
        fs.readFileSync(path.join(this.home, ".zdev-onboarded"), "utf8").trim() ===
        ONBOARDING_VERSION
      );
    } catch {
      return false;
    }
  }

  markOnboarded(): void {
    fs.mkdirSync(this.home, { recursive: true });
    fs.writeFileSync(
      path.join(this.home, ".zdev-onboarded"),
      ONBOARDING_VERSION + "\n",
      "utf8",
    );
  }

  /** `user_name = "…"` in config.toml — drives "Welcome back <name>". */
  userName(): string {
    const cfg = store.loadConfig(this.home);
    return (cfg.user_name as string) ?? "";
  }

  persistName(name: string): void {
    const p = path.join(this.home, "config.toml");
    let text = "";
    try {
      text = fs.readFileSync(p, "utf8");
    } catch {}
    const line = `user_name = ${JSON.stringify(name)}`;
    text = /^user_name\s*=/m.test(text)
      ? text.replace(/^user_name\s*=.*$/m, line)
      : line + "\n" + text;
    fs.mkdirSync(this.home, { recursive: true });
    fs.writeFileSync(p, text, "utf8");
  }

  /** Append `[providers.X]` + `[models.alias]` blocks; skip what exists. */
  persistProvider(p: {
    name: string;
    kind: string;
    base_url: string;
    api_key_env: string;
    alias: string;
    model_id: string;
    context: number;
  }): void {
    const f = path.join(this.home, "config.toml");
    let text = "";
    try {
      text = fs.readFileSync(f, "utf8");
    } catch {}
    let add = "";
    if (!new RegExp(`^\\s*\\[providers\\.${p.name}\\]`, "m").test(text)) {
      add += `\n[providers.${p.name}]\nkind = "${p.kind}"\n`;
      if (p.base_url) add += `base_url = "${p.base_url}"\n`;
      if (p.api_key_env) add += `api_key_env = "${p.api_key_env}"\n`;
    }
    if (!new RegExp(`^\\s*\\[models\\.${p.alias}\\]`, "m").test(text)) {
      add +=
        `\n[models.${p.alias}]\nprovider = "${p.name}"\n` +
        `id = "${p.model_id}"\ncontext = ${p.context}\n`;
    }
    if (!add) return;
    fs.mkdirSync(this.home, { recursive: true });
    fs.writeFileSync(f, text + add, "utf8");
  }

  /** Persist a BYOK key as a User-scope env var (registry HKCU\Environment)
   * + into this process — config.toml stays key-free per contract §12. */
  persistEnvVar(name: string, value: string): void {
    process.env[name] = value;
    if (process.platform !== "win32") return;
    spawnSync(
      "reg",
      ["add", "HKCU\\Environment", "/v", name, "/t", "REG_SZ", "/d", value, "/f"],
      { windowsHide: true },
    );
  }

  sessions(): Record<string, any>[] {
    return store.Session.list(this.home).slice().reverse(); // newest first
  }

  async oauthLogin(onOpen: (url: string) => void): Promise<{
    email?: string;
  }> {
    const { loginChatGPT } = await import("../oauth");
    const a = await loginChatGPT(this.home, onOpen);
    return { email: a.email };
  }

  // ── AcpClient-shaped surface ───────────────────────────────────────────
  async call(method: string, params: any): Promise<any> {
    try {
      return { result: await this.dispatch(method, params ?? {}) };
    } catch (e: any) {
      return { error: { code: -32603, message: String(e?.message ?? e) } };
    }
  }

  notify(method: string, params: any) {
    if (method === "session/cancel") this.cancelFlag.set = true;
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

  private emit = (sid: string) => (ev: proto.Event) => {
    const n = updateFor(sid, ev);
    if (n) this.onUpdate(sid, n.params.update);
  };

  private async dispatch(method: string, params: Json): Promise<Json> {
    switch (method) {
      case "initialize":
        return {
          protocolVersion: 1,
          agentInfo: { name: "zdev", version: "0.1.0" },
          agentCapabilities: {
            loadSession: false,
            promptCapabilities: { embeddedContext: true },
          },
        };
      case "session/new": {
        const o = new proto.RunOpts();
        Object.assign(o, this.opts);
        o.cwd = params.cwd ?? this.opts.cwd ?? this.cwd;
        if (["plan", "agent", "yolo"].includes(params.mode)) {
          o.mode = params.mode;
          o.yolo = params.mode === "yolo";
        }
        if (params.model) o.model = params.model;
        if (params.resume) o.resume = params.resume;
        const a = new Agent(o, "zdev", o.cwd ?? this.cwd);
        this.agent = a;
        this.cancelFlag = { set: false };
        this.askerCtx = {};
        a.asker = this.makeAsker(a);
        return {
          sessionId: a.session.id,
          model: a.modelAlias,
          mode: a.mode,
        };
      }
      case "session/prompt": {
        const a = this.mustAgent();
        this.cancelFlag.set = false;
        const prompt = (params.prompt ?? [])
          .map((b: Json) => b.text ?? "")
          .join("\n");
        const emit = this.emit(a.session.id);
        a.sendUser(prompt, emit);
        const reason = await this.runTurns(a, emit);
        return { stopReason: reason };
      }
      case "session/approve": {
        const a = this.mustAgent();
        this.cancelFlag.set = false;
        const emit = this.emit(a.session.id);
        a.approvePlan(emit);
        const reason = await this.runTurns(a, emit);
        return { stopReason: reason };
      }
      case "session/set_mode": {
        const a = this.mustAgent();
        const mode = params.mode ?? "";
        if (!["plan", "agent", "yolo"].includes(mode)) {
          throw new Error(`bad mode ${JSON.stringify(mode)} (plan|agent|yolo)`);
        }
        const old = a.mode;
        a.mode = mode;
        a.opts.yolo = mode === "yolo";
        a.session.append("mode", { from: old, to: mode });
        return { mode };
      }
      case "session/set_model": {
        const a = this.mustAgent();
        const alias = params.model ?? "";
        // reload config — onboarding may have written providers since boot
        a.cfg = store.loadConfig(this.home);
        const found = store.providerFor(a.cfg, alias);
        if (!found) throw new Error(`unknown model ${JSON.stringify(alias)}`);
        const [provider, modelId] = found;
        a.provider = provider;
        a.allowed = store.allowedHosts(a.cfg);
        a.modelAlias = `${alias} (${modelId})`;
        a.session.append("model_switch", { alias, id: modelId });
        return { model: a.modelAlias };
      }
      case "session/list_models": {
        const cfg = store.loadConfig(this.home);
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
        return { models, default: cfg.default_model };
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
      const result = await this.requestFromClient("session/request_permission", {
        sessionId: a.session.id,
        toolCallId: this.askerCtx.tid ?? tool,
        tool,
        input: this.askerCtx.input ?? {},
        options: [
          { id: "allow", name: "Allow once" },
          { id: "allow_always", name: "Always allow" },
          { id: "deny", name: "Deny" },
        ],
      });
      const outcome = result?.outcome ?? {};
      const opt = outcome.optionId ?? outcome.outcome ?? "deny";
      if (opt === "allow_always" || opt === "always") {
        if (!a.opts.allow.includes(tool)) a.opts.allow.push(tool);
        return true;
      }
      return opt === "allow" || opt === "selected" || opt === "yes";
    };
  }

  private async runTurns(
    a: Agent,
    emit: (ev: proto.Event) => void,
  ): Promise<string> {
    for (let i = 0; i < (a.opts.max_turns ?? 16); i++) {
      if (this.cancelFlag.set) return "cancelled";
      let done: boolean;
      try {
        done = await a.turn(emit);
      } catch (e: any) {
        this.stderrTail.push(String(e?.message ?? e));
        return "refusal";
      }
      if (done) return "end_turn";
      const calls = (a.messages[a.messages.length - 1].content ?? []).filter(
        (b) => b.type === "tool_use",
      );
      if (!calls.length) return "end_turn";
      for (const c of calls) {
        if (this.cancelFlag.set) return "cancelled";
        this.askerCtx.tid = c.id;
        this.askerCtx.input = c.input;
        await a.execTool(c.id, c.name, c.input, emit);
        if (c.name === "submit_plan") return "end_turn";
      }
    }
    return "max_turns";
  }
}
