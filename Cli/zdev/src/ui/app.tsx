// App — the zdev TUI. History is committed to the terminal's own scrollback
// through Ink's <Static> (the welcome box first, then every settled row —
// finished paragraphs of a streaming reply included), so the live region
// under it stays small: the growing tail of the reply, running tools, the
// composer and whatever menu is open. The agent runs in-process behind
// ZdevDriver, which mirrors the ACP surface.
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { spawn } from "node:child_process";
import { Box, Static, Text, useApp, useStdin, useStdout } from "ink";

import type { ModelDef, ProviderDef } from "../catalog";
import { Composer } from "../components/Composer";
import { Permission } from "../components/Permission";
import { Picker, type Row } from "../components/Picker";
import { Thinking } from "../components/Thinking";
import { HeaderBox, TranscriptItem, type ReadCache } from "../components/Transcript";
import { parseReadDetail } from "../diff";
import { RejectedKey } from "../live";
import { PINNED, pitchFor } from "../pinned";
import * as devin from "../devin";
import type { RunOpts } from "../proto";
import { applyUpdate, settle, summarizeArg } from "../reducer";
import { DIM, GRAY, GREEN, ROSE } from "../theme";
import * as t from "../tools";
import type { Item, Model, PermissionReq, Usage } from "../types";
import { emptyState } from "../types";
import { ZdevDriver } from "./driver";
import { KeyParser, lineEnd, lineStart, verticalMove, wordLeft, wordRight, type Key } from "./keys";
import {
  appendHistory,
  copyText,
  expandMentions,
  fileIndex,
  loadCommands,
  loadHistory,
  mentionAt,
  parseSlash,
  rank,
  type Slash,
} from "./menus";

let nid = 0;
const nextId = () => `a${++nid}`;
const note = (text: string, tone?: "ok" | "warn" | "error"): Item => ({ kind: "note", id: nextId(), text, tone });

export type Boot = { cwd: string; model?: string; mode?: string; opts: RunOpts };
export type ExitReason = "quit" | "onboarding";

const MODES = ["plan", "agent", "accept-edits", "yolo"];
const EFFORTS = ["", "low", "medium", "high"];
const INIT_PROMPT =
  "Explore this repository and write an AGENTS.md at its root for coding agents: what the project is, how the code is laid out, how to build, run, test and lint it, the conventions to follow, and anything surprising. Read the files you describe; keep it short and accurate. If AGENTS.md exists, improve it instead of replacing it.";
const REVIEW_PROMPT =
  "Review the uncommitted changes in this repository (use `git status` and `git diff`, plus `git diff --staged`). Report real problems first — bugs, security issues, broken edge cases, missing tests — each with file:line and a concrete fix. Skip praise and style nits. Don't modify files.";

type Connect =
  | { step: "provider"; query: string; sel: number }
  | { step: "key"; provider: ProviderDef; key: string; error?: string }
  | { step: "wait"; provider: ProviderDef; message: string; url?: string }
  | { step: "models"; provider: ProviderDef; models: ModelDef[]; query: string; sel: number; source: string };

type Popup =
  | { kind: "model"; models: Model[]; query: string; sel: number; effort: string; loading?: string }
  | { kind: "connect"; c: Connect }
  | { kind: "resume"; sessions: Record<string, any>[]; sel: number }
  | { kind: "help"; scroll: number };

type Transcript = { done: Item[]; live: Item[]; usage: Usage; key: number };

const fmtK = (n: number) => (n >= 1_000_000 ? `${(n / 1_048_576).toFixed(n % 1_048_576 ? 1 : 0)}M` : n >= 1000 ? `${Math.round(n / 1000)}k` : String(n));
const money = (x: number) => `$${x.toFixed(2).replace(/\.?0+$/, "")}`;

function priceText(m: { price: Model["price"]; free: boolean; kind: string }): string {
  if (m.kind === "chatgpt" || m.kind === "devin") return "plan";
  if (m.free) return "free";
  return `${money(m.price.input)}/${money(m.price.output)}`;
}

/** Rebuild transcript rows from a resumed session's provider messages. */
function replayItems(messages: { role: string; content: Record<string, any>[] }[]): Item[] {
  const items: Item[] = [];
  const tools = new Map<string, Extract<Item, { kind: "tool" }>>();
  for (const m of messages) {
    for (const b of m.content) {
      if (b.type === "text" && b.text) {
        items.push(m.role === "assistant" ? { kind: "assistant", id: nextId(), text: b.text } : { kind: "user", id: nextId(), text: b.text });
      } else if (b.type === "tool_use") {
        const it = {
          kind: "tool" as const,
          id: nextId(),
          toolCallId: b.id,
          name: b.name,
          arg: summarizeArg(b.name, b.input),
          status: "completed" as const,
          input: b.input,
        };
        tools.set(b.id, it);
        items.push(it);
      } else if (b.type === "tool_result") {
        const it = tools.get(b.tool_use_id);
        if (it) {
          it.detail = String(b.content ?? "");
          if (b.is_error) (it as any).status = "failed";
        }
      }
    }
  }
  return items;
}

export function App({ boot, onExit, onDriver }: { boot: Boot; onExit: (r: ExitReason) => void; onDriver?: (d: ZdevDriver) => void }) {
  const { exit } = useApp();
  const { stdout } = useStdout();
  const { internal_eventEmitter: stdinEvents, setRawMode, isRawModeSupported } = useStdin() as any;
  const [cols, setCols] = useState(stdout?.columns ?? 100);
  const width = Math.max(40, cols - 1);

  const driver = useMemo(() => new ZdevDriver(boot.opts, boot.cwd), [boot]);
  onDriver?.(driver); // tests reach the backend through this
  const [tr, setTr] = useState<Transcript>({ done: [], live: [], usage: emptyState().usage, key: 0 });
  const [sessionId, setSessionId] = useState("");
  const [model, setModel] = useState(boot.model ?? "…");
  const [mode, setMode] = useState(boot.mode ?? "agent");
  const [ctxLimit, setCtxLimit] = useState(0);
  const [busy, setBusy] = useState(false);
  const [buf, setBuf] = useState("");
  const [cur, setCur] = useState(0);
  const [menuSel, setMenuSel] = useState(0);
  const [perm, setPerm] = useState<PermissionReq | null>(null);
  const [permSel, setPermSel] = useState(0);
  const [popup, setPopup] = useState<Popup | null>(null);
  const [verbose, setVerbose] = useState(false);
  const [listening, setListening] = useState(false);
  const [flash, setFlash] = useState<{ text: string; tone?: "ok" | "error"; at: number } | null>(null);
  const [quitArmed, setQuitArmed] = useState(0);
  const [files, setFiles] = useState<string[] | null>(null);

  const busyRef = useRef(false);
  const history = useRef<string[]>(loadHistory());
  const histIdx = useRef(-1);
  const draft = useRef("");
  const pastes = useRef<string[]>([]);
  const pendingCtx = useRef<string[]>([]);
  const lastReply = useRef("");
  const turnReply = useRef("");
  const connectAbort = useRef<AbortController | null>(null);
  const commands = useMemo<Slash[]>(() => loadCommands(boot.cwd), [boot.cwd]);
  const user = driver.userName() || process.env.USERNAME || process.env.USER || "";

  // ── transcript plumbing ───────────────────────────────────────────────
  /** Apply `f` to the live rows, then commit whatever settled. */
  const update = useCallback((f: (live: Item[], usage: Usage) => { live: Item[]; usage?: Usage }) => {
    setTr((s) => {
      const r = f(s.live, s.usage);
      const st = settle(r.live, busyRef.current);
      return { ...s, done: st.done.length ? [...s.done, ...st.done] : s.done, live: st.live, usage: r.usage ?? s.usage };
    });
  }, []);
  const push = useCallback((...items: Item[]) => update((live) => ({ live: [...live, ...items] })), [update]);
  const say = useCallback((text: string, tone?: "ok" | "error") => setFlash({ text, tone, at: Date.now() }), []);

  useEffect(() => {
    if (!flash) return;
    const id = setTimeout(() => setFlash(null), 6000);
    return () => clearTimeout(id);
  }, [flash]);

  useEffect(() => {
    const onResize = () => setCols(stdout?.columns ?? 100);
    stdout?.on("resize", onResize);
    return () => {
      stdout?.off("resize", onResize);
    };
  }, [stdout]);

  // ── boot ──────────────────────────────────────────────────────────────
  const startSession = useCallback(
    async (params: Record<string, any> = {}) => {
      const sn: any = await driver.call("session/new", {
        cwd: boot.cwd,
        ...(boot.model && !params.resume ? { model: boot.model } : {}),
        ...(boot.mode ? { mode: boot.mode } : {}),
        ...params,
      });
      if (sn.error) throw new Error(sn.error.message);
      setSessionId(sn.result.sessionId);
      setModel(sn.result.model ?? "?");
      setMode(sn.result.mode ?? "agent");
      setCtxLimit(sn.result.context ?? 0);
      return sn.result;
    },
    [boot, driver],
  );

  useEffect(() => {
    driver.onUpdate = (_sid, u) => {
      if (u?.sessionUpdate === "agent_message_chunk") turnReply.current += u.content?.text ?? "";
      update((live, usage) => {
        const s = applyUpdate({ items: live, usage }, u);
        return { live: s.items, usage: s.usage };
      });
    };
    driver.onRequest = (req) => {
      if (req.method === "session/request_permission") {
        setPermSel(0);
        setPerm({
          id: req.id,
          toolCallId: req.params?.toolCallId ?? "",
          tool: req.params?.tool ?? "tool",
          input: req.params?.input ?? {},
          options: req.params?.options ?? [],
        });
      } else driver.reply(req.id, undefined);
    };
    (async () => {
      try {
        const r = await startSession();
        setTr((s) => ({ ...s, done: [{ kind: "header", id: "header" }, ...s.done] }));
        if (/^fixture\b/.test(r.model ?? "")) {
          push(note("No model connected yet — /connect adds an API key, /login signs in with ChatGPT, /model picks one.", "warn"));
        }
      } catch (e: any) {
        setTr((s) => ({ ...s, done: [{ kind: "header", id: "header" }] }));
        push(note(`Couldn't start a session: ${e?.message ?? e}`, "error"));
      }
    })();
    // the @file index builds in the background
    setTimeout(() => setFiles(fileIndex(boot.cwd)), 50);
    return () => driver.kill();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const quit = useCallback(
    (reason: ExitReason = "quit") => {
      connectAbort.current?.abort();
      driver.kill();
      onExit(reason);
      exit();
    },
    [driver, exit, onExit],
  );

  // ── prompts ───────────────────────────────────────────────────────────
  const runPrompt = useCallback(
    async (method: "session/prompt" | "session/approve", text?: string) => {
      if (!sessionId || busyRef.current) return;
      busyRef.current = true;
      setBusy(true);
      turnReply.current = "";
      const t0 = Date.now();
      let cancelled = false;
      try {
        const res: any = await driver.call(method, method === "session/prompt" ? { sessionId, prompt: [{ type: "text", text }] } : { sessionId });
        const reason = res?.result?.stopReason;
        if (res?.error) push(note(res.error.message, "error"));
        cancelled = reason === "cancelled";
        if (reason === "max_turns") push(note("Stopped after the turn limit — say “continue” to keep going.", "warn"));
      } catch (e: any) {
        push(note(`backend: ${e?.message ?? e}`, "error"));
      } finally {
        if (turnReply.current.trim()) lastReply.current = turnReply.current;
        busyRef.current = false;
        setBusy(false);
        setPerm(null);
        setTr((s) => {
          const out = (s.usage.output || 0) as number;
          const live = [...s.live, { kind: "turn" as const, id: nextId(), secs: (Date.now() - t0) / 1000, out: out || undefined, cancelled }];
          const st = settle(live, false);
          return { ...s, done: [...s.done, ...st.done], live: st.live };
        });
      }
    },
    [driver, push, sessionId],
  );

  const sendPrompt = useCallback(
    (shown: string, full?: string) => {
      let text = expandMentions(full ?? shown, boot.cwd);
      if (pendingCtx.current.length) {
        text = `${pendingCtx.current.join("\n\n")}\n\n${text}`;
        pendingCtx.current = [];
      }
      push({ kind: "user", id: nextId(), text: shown });
      void runPrompt("session/prompt", text);
    },
    [boot.cwd, push, runPrompt],
  );

  const cancel = useCallback(() => {
    if (!busyRef.current) return;
    driver.notify("session/cancel", { sessionId });
    setPerm(null);
  }, [driver, sessionId]);

  const setModeTo = useCallback(
    async (m: string) => {
      const res: any = await driver.call("session/set_mode", { sessionId, mode: m });
      if (res?.error) say(res.error.message, "error");
      else setMode(m);
    },
    [driver, say, sessionId],
  );

  const setModelTo = useCallback(
    async (alias: string, effort = "") => {
      const res: any = await driver.call("session/set_model", { sessionId, model: alias, remember: true, ...(effort ? { effort } : {}) });
      if (res?.error) return say(res.error.message, "error");
      setModel(res.result.model);
      if (res.result.context) setCtxLimit(res.result.context);
      say(`Model → ${alias}${effort ? ` · ${effort} effort` : ""} (saved as your default)`, "ok");
    },
    [driver, say, sessionId],
  );

  // ── pickers ───────────────────────────────────────────────────────────
  const openModelPicker = useCallback(async () => {
    const res: any = await driver.call("session/list_models", {});
    const models: Model[] = res?.result?.models ?? [];
    const curAlias = model.split(" ")[0];
    setPopup({ kind: "model", models, query: "", sel: Math.max(0, models.findIndex((m) => m.alias === curAlias)), effort: "" });
  }, [driver, model]);

  const fetchForConnect = useCallback(
    async (p: ProviderDef, justStored: boolean) => {
      const ac = new AbortController();
      connectAbort.current = ac;
      const host = p.base_url.split("/")[2] || p.name;
      setPopup({ kind: "connect", c: { step: "wait", provider: p, message: `Checking your key and fetching live models from ${host}…` } });
      try {
        const models = await driver.fetchModels(p.id, ac.signal);
        setPopup({ kind: "connect", c: { step: "models", provider: p, models, query: "", sel: 0, source: `${models.length} models live from ${host}` } });
      } catch (e: any) {
        if (ac.signal.aborted) return;
        if (e instanceof RejectedKey && justStored) {
          driver.deleteKey(p.id);
          setPopup({ kind: "connect", c: { step: "key", provider: p, key: "", error: `${e.message} — check it and paste again` } });
          return;
        }
        setPopup({
          kind: "connect",
          c: { step: "models", provider: p, models: p.models, query: "", sel: 0, source: `couldn't fetch the live list (${e?.message ?? e}) — showing known models` },
        });
      }
    },
    [driver],
  );

  const connectProvider = useCallback(
    (p: ProviderDef) => {
      if (p.kind === "chatgpt") {
        const ac = new AbortController();
        connectAbort.current = ac;
        setPopup({ kind: "connect", c: { step: "wait", provider: p, message: "Signing in with ChatGPT — finish in your browser" } });
        driver
          .login((url) => setPopup({ kind: "connect", c: { step: "wait", provider: p, message: "Signing in with ChatGPT — finish in your browser", url } }), ac.signal)
          .then((tok) => {
            say(`Signed in${tok.email ? ` as ${tok.email}` : ""}${tok.plan ? ` · ${tok.plan}` : ""}`, "ok");
            void fetchForConnect(p, false);
          })
          .catch((e) => {
            if (ac.signal.aborted) return;
            setPopup(null);
            say(`Sign-in failed: ${e?.message ?? e}`, "error");
          });
        return;
      }
      if (p.kind === "devin") {
        devin.invalidate();
        if (devin.ready()) void fetchForConnect(p, false);
        else {
          setPopup(null);
          say(devin.hint(), "error");
        }
        return;
      }
      const ready = driver.providers().find((x) => x.def.id === p.id)?.ready;
      if (ready) void fetchForConnect(p, false);
      else setPopup({ kind: "connect", c: { step: "key", provider: p, key: "" } });
    },
    [driver, fetchForConnect, say],
  );

  const providerRows = useCallback(
    (query: string) => {
      const list = driver.providers();
      const pinRank = (id: string) => {
        const i = PINNED.findIndex((x) => x.id === id);
        return i < 0 ? PINNED.length : i;
      };
      list.sort((a, b) => pinRank(a.def.id) - pinRank(b.def.id));
      const order = rank(list, query, (x) => `${x.def.id} ${x.def.name} ${pitchFor(x.def)}`);
      return order.map((i) => list[i]);
    },
    [driver],
  );

  // ── slash commands ────────────────────────────────────────────────────
  const runShell = useCallback(
    (cmd: string) => {
      const id = nextId();
      push({ kind: "shell", id, cmd, out: "", code: null, pending: true });
      const bash = (process.env.PATH ?? "").split(process.platform === "win32" ? ";" : ":").some((d) => {
        try {
          return require("node:fs").existsSync(`${d}\\bash.exe`);
        } catch {
          return false;
        }
      });
      const child =
        process.platform === "win32" && !bash
          ? spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", cmd], { cwd: boot.cwd, windowsHide: true })
          : spawn(process.platform === "win32" ? "bash.exe" : "bash", ["-lc", cmd], { cwd: boot.cwd, windowsHide: true });
      let out = "";
      child.stdout?.on("data", (d) => (out += d.toString()));
      child.stderr?.on("data", (d) => (out += d.toString()));
      const done = (code: number | null) => {
        update((live) => ({
          live: live.map((it) => (it.id === id && it.kind === "shell" ? { ...it, out, code, pending: false } : it)),
        }));
        // the output rides along with the next prompt, like Claude Code's `!`
        pendingCtx.current.push(`<shell-output command=${JSON.stringify(cmd)} exit=${code}>\n${out.slice(0, 8000)}\n</shell-output>`);
      };
      child.on("error", (e) => {
        out += String(e.message);
        done(127);
      });
      child.on("close", (code) => done(code));
    },
    [boot.cwd, push, update],
  );

  const gitBlock = useCallback(() => {
    const run = (args: string[]) => {
      const r = require("node:child_process").spawnSync("git", args, { cwd: boot.cwd, encoding: "utf8", windowsHide: true });
      return r.status === 0 ? String(r.stdout ?? "") : "";
    };
    const status = run(["status", "--short"]);
    const stat = run(["diff", "--stat", "HEAD"]);
    if (!status.trim() && !stat.trim()) return say("Working tree is clean (or not a git repository)");
    push({ kind: "shell", id: nextId(), cmd: "git status --short && git diff --stat HEAD", out: `${status}${stat}`, code: 0 });
  }, [boot.cwd, push, say]);

  const runSlash = useCallback(
    async (name: string, args: string) => {
      switch (name) {
        case "model":
          if (args) void setModelTo(args);
          else void openModelPicker();
          return;
        case "providers":
        case "connect": {
          if (args) {
            const p = driver.providers().find((x) => x.def.id === args || x.def.name.toLowerCase() === args.toLowerCase());
            if (p) return connectProvider(p.def);
            say(`Unknown provider: ${args}`, "error");
          }
          setPopup({ kind: "connect", c: { step: "provider", query: "", sel: 0 } });
          return;
        }
        case "login": {
          const p = driver.providers().find((x) => x.def.kind === "chatgpt");
          if (p) connectProvider(p.def);
          return;
        }
        case "logout": {
          const id = args || "chatgpt";
          say(await driver.logout(id), "ok");
          return;
        }
        case "mode":
          if (!args) return say(`Mode: ${mode} — /mode plan|agent|accept-edits|yolo, or shift+tab to cycle`);
          if (!MODES.includes(args)) return say(`Unknown mode ${args} — plan, agent, accept-edits or yolo`, "error");
          return void setModeTo(args);
        case "plan":
        case "agent":
        case "yolo":
        case "accept-edits":
          return void setModeTo(name);
        case "approve":
          return void runPrompt("session/approve");
        case "compact": {
          const focus = args.replace(/^focus:?\s*/, "");
          driver.agent?.compactNow(focus || undefined, () => {});
          return say(focus ? `Compacted — focus: ${focus}` : "Compacted — world.md saved", "ok");
        }
        case "undo": {
          const msg = driver.agent?.undoLast(() => {}) ?? "no session";
          return say(msg, msg.startsWith("undo") ? "error" : "ok");
        }
        case "skillify": {
          const msg = driver.agent?.skillify(args || "session", () => {}) ?? "no session";
          return say(msg, msg.includes("wrote") ? "ok" : "error");
        }
        case "refine":
          return say("Refine proposes a bounded skill edit via meridian refine.* — use /skillify first", "ok");
        case "usage": {
          const u = tr.usage;
          const rl = driver.rateLimits();
          const lines = [
            `**Session usage** — ${model}`,
            "",
            `- last turn: ${u.input.toLocaleString()} in · ${u.output.toLocaleString()} out · ${u.cache_read.toLocaleString()} cached`,
            `- cost so far: ${u.cum_usd != null && u.cum_usd > 0 ? `$${u.cum_usd.toFixed(4)}` : "—"}`,
          ];
          if (rl) {
            lines.push(`- ChatGPT${rl.plan ? ` ${rl.plan}` : ""} plan: ${rl.primaryUsed != null ? `${Math.round(100 - rl.primaryUsed)}% of the 5-hour window left` : ""}${rl.secondaryUsed != null ? ` · ${Math.round(100 - rl.secondaryUsed)}% of the week left` : ""}`);
          }
          return push({ kind: "assistant", id: nextId(), text: lines.join("\n") });
        }
        case "cost": {
          const u = tr.usage;
          return push({
            kind: "assistant",
            id: nextId(),
            text: `**Session cost** — input ${u.input.toLocaleString()} · output ${u.output.toLocaleString()} · cache read ${u.cache_read.toLocaleString()} · cache write ${u.cache_write.toLocaleString()} · total ${u.cum_usd != null ? `$${u.cum_usd.toFixed(4)}` : "unpriced"}`,
          });
        }
        case "context": {
          const used = tr.usage.input + tr.usage.cache_read + tr.usage.cache_write;
          if (!ctxLimit) return say(`~${used.toLocaleString()} tokens in context (window size unknown for this model)`);
          const pct = used / ctxLimit;
          const bar = "■".repeat(Math.round(pct * 30)) + "□".repeat(30 - Math.round(pct * 30));
          return push({ kind: "assistant", id: nextId(), text: `**Context** \`${bar}\` ${Math.round(pct * 100)}%\n\n${used.toLocaleString()} of ${ctxLimit.toLocaleString()} tokens used. Compaction kicks in at 80%.` });
        }
        case "resume": {
          const sessions = driver.sessions().slice(0, 50);
          if (!sessions.length) return say("No earlier sessions to resume");
          return setPopup({ kind: "resume", sessions, sel: 0 });
        }
        case "init":
          store.initProject(driver.agent?.workspace ?? process.cwd());
          return sendPrompt("/init", INIT_PROMPT);
        case "review":
          return sendPrompt(args ? `/review ${args}` : "/review", REVIEW_PROMPT + (args ? ` Focus: ${args}` : ""));
        case "diff":
          return gitBlock();
        case "copy":
          if (!lastReply.current) return say("Nothing to copy yet");
          copyText(lastReply.current);
          return say(`Copied the last reply (${lastReply.current.length} chars)`, "ok");
        case "voice":
          return dictate();
        case "session":
          return say(`Session ${sessionId} · resume with  zdev --resume ${sessionId}`);
        case "clear": {
          stdout.write("\x1b[2J\x1b[3J\x1b[H");
          try {
            await startSession();
          } catch (e: any) {
            say(String(e?.message ?? e), "error");
          }
          setTr((s) => ({ done: [{ kind: "header", id: "header" }], live: [], usage: emptyState().usage, key: s.key + 1 }));
          return;
        }
        case "onboarding":
          return quit("onboarding");
        case "help":
          return setPopup({ kind: "help", scroll: 0 });
        case "quit":
        case "exit":
        case "q":
          return quit();
        default: {
          const c = commands.find((x) => x.name === name && x.body);
          if (c) return sendPrompt(args ? `/${name} ${args}` : `/${name}`, c.body!.replace(/\$ARGUMENTS/g, args));
          say(`Unknown command: /${name}`, "error");
        }
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [commands, connectProvider, ctxLimit, driver, gitBlock, mode, model, openModelPicker, push, quit, runPrompt, say, sendPrompt, sessionId, setModeTo, setModelTo, tr.usage],
  );

  const dictate = useCallback(() => {
    if (listening) return;
    setListening(true);
    void import("../voice")
      .then((v) => v.dictate())
      .then((r) => {
        setListening(false);
        if (r.error) return say(`Mic: ${r.error}`, "error");
        if (r.text) insertText((buf && !buf.endsWith(" ") ? " " : "") + r.text);
        else say("Didn't catch anything — try again");
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listening, buf]);

  // ── menus under the composer ──────────────────────────────────────────
  const slashMenu = useMemo(() => {
    const m = buf.match(/^\/(\S*)$/);
    if (!m || popup || perm) return null;
    const order = rank(commands, m[1], (c) => c.name);
    return order.map((i) => commands[i]);
  }, [buf, commands, perm, popup]);

  const fileMenu = useMemo(() => {
    if (slashMenu || popup || perm || !files) return null;
    const at = mentionAt(buf, cur);
    if (!at) return null;
    const order = rank(files, at.query, (f) => f).slice(0, 50);
    return { at, list: order.map((i) => files[i]) };
  }, [buf, cur, files, perm, popup, slashMenu]);

  useEffect(() => setMenuSel(0), [slashMenu?.length, fileMenu?.at.query]);

  // ── editing ───────────────────────────────────────────────────────────
  const setInput = (b: string, c: number) => {
    setBuf(b);
    setCur(Math.max(0, Math.min(c, b.length)));
  };
  function insertText(s: string) {
    setBuf((b) => {
      const nb = b.slice(0, cur) + s + b.slice(cur);
      return nb;
    });
    setCur((c) => c + s.length);
  }

  const onPaste = (raw: string) => {
    const s = raw.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
    if (popup?.kind === "connect" && popup.c.step === "key") {
      const c = popup.c;
      return setPopup({ ...popup, c: { ...c, key: c.key + s.trim(), error: undefined } });
    }
    if (popup) return;
    const lines = s.split("\n").length;
    if (lines > 8 || s.length > 2000) {
      pastes.current.push(s);
      insertText(`[Pasted text #${pastes.current.length} +${lines} lines]`);
    } else insertText(s);
  };

  const expandPastes = (text: string) =>
    pastes.current.reduce((out, p, i) => out.replace(`[Pasted text #${i + 1} +${p.split("\n").length} lines]`, p), text);

  const submit = () => {
    const raw = buf.trim();
    if (!raw) return;
    if (busyRef.current && !raw.startsWith("/")) {
      return say("zdev is working — esc interrupts, then send your message");
    }
    setInput("", 0);
    histIdx.current = -1;
    if (history.current[history.current.length - 1] !== raw) {
      history.current.push(raw);
      appendHistory(raw);
    }
    const sl = parseSlash(raw);
    if (sl) return void runSlash(sl[0], sl[1]);
    if (raw.startsWith("!")) {
      const cmd = raw.slice(1).trim();
      if (cmd) runShell(cmd);
      return;
    }
    if (raw.startsWith("#")) {
      const text = raw.slice(1).trim();
      if (!text) return;
      const r = t.run(new t.Ctx(boot.cwd), "memory_write", { type: "preference", text, scope: "zdev" });
      return r.startsWith("ERROR") ? say(`Memory: ${r.replace(/^ERROR:\s*/, "")}`, "error") : say(`Remembered — “${text.slice(0, 80)}”`, "ok");
    }
    const full = expandPastes(raw);
    pastes.current = [];
    sendPrompt(raw, full);
  };

  // ── keys ──────────────────────────────────────────────────────────────
  const onPopupKey = (k: Key): boolean => {
    if (!popup) return false;
    const typing = (q: string) => (k.name === "text" && k.text ? q + k.text : k.name === "backspace" ? q.slice(0, -1) : null);
    const move = (sel: number, n: number) =>
      k.name === "up" ? (sel - 1 + n) % Math.max(1, n) : k.name === "down" ? (sel + 1) % Math.max(1, n) : k.name === "pageup" ? Math.max(0, sel - 8) : k.name === "pagedown" ? Math.min(n - 1, sel + 8) : null;

    if (popup.kind === "help") {
      const step = k.name === "up" ? -1 : k.name === "down" ? 1 : k.name === "pageup" ? -10 : k.name === "pagedown" ? 10 : 0;
      if (step) setPopup({ kind: "help", scroll: Math.max(0, popup.scroll + step) });
      else setPopup(null); // any other key closes
      return true;
    }
    if (popup.kind === "resume") {
      const n = popup.sessions.length;
      if (k.name === "escape") setPopup(null);
      const m = move(popup.sel, n);
      if (m !== null) setPopup({ ...popup, sel: m });
      if (k.name === "return" && n) {
        const s = popup.sessions[popup.sel];
        setPopup(null);
        void (async () => {
          try {
            await startSession({ resume: s.id });
            const msgs = driver.agent?.messages ?? [];
            stdout.write("\x1b[2J\x1b[3J\x1b[H");
            setTr((st) => ({
              done: [{ kind: "header", id: "header" }, ...replayItems(msgs), note(`Resumed session ${s.id.slice(0, 8)} — ${msgs.length} messages of context restored`, "ok")],
              live: [],
              usage: emptyState().usage,
              key: st.key + 1,
            }));
          } catch (e: any) {
            say(`Resume failed: ${e?.message ?? e}`, "error");
          }
        })();
      }
      return true;
    }
    if (popup.kind === "model") {
      const order = rank(popup.models, popup.query, (m) => `${m.alias} ${m.name} ${m.providerName}`);
      const n = order.length;
      if (k.name === "escape") return setPopup(null), true;
      const m = move(popup.sel, n);
      if (m !== null) return setPopup({ ...popup, sel: m }), true;
      if (k.name === "left" || k.name === "right") {
        const i = EFFORTS.indexOf(popup.effort);
        const j = (i + (k.name === "right" ? 1 : EFFORTS.length - 1)) % EFFORTS.length;
        return setPopup({ ...popup, effort: EFFORTS[j] }), true;
      }
      if (k.ctrl && k.name === "r" && n) {
        const pick = popup.models[order[popup.sel]];
        if (pick.fromConfig) return say("That model is defined in config.toml — no live list"), true;
        setPopup({ ...popup, loading: `Fetching live models from ${pick.providerName}…` });
        driver
          .fetchModels(pick.provider)
          .then(async () => {
            const res: any = await driver.call("session/list_models", {});
            setPopup((p) => (p?.kind === "model" ? { ...p, models: res?.result?.models ?? p.models, loading: undefined } : p));
            say(`Refreshed ${pick.providerName}'s live model list`, "ok");
          })
          .catch((e) => {
            setPopup((p) => (p?.kind === "model" ? { ...p, loading: undefined } : p));
            say(String(e?.message ?? e), "error");
          });
        return true;
      }
      if (k.name === "return" && n) {
        const pick = popup.models[order[Math.min(popup.sel, n - 1)]];
        setPopup(null);
        void setModelTo(pick.alias, popup.effort);
        return true;
      }
      const q = typing(popup.query);
      if (q !== null) setPopup({ ...popup, query: q, sel: 0 });
      return true;
    }
    // connect
    const c = popup.c;
    if (c.step === "provider") {
      const list = providerRows(c.query);
      if (k.name === "escape") return setPopup(null), true;
      const m = move(c.sel, list.length);
      if (m !== null) return setPopup({ kind: "connect", c: { ...c, sel: m } }), true;
      if (k.name === "return" && list.length) return connectProvider(list[Math.min(c.sel, list.length - 1)].def), true;
      const q = typing(c.query);
      if (q !== null) setPopup({ kind: "connect", c: { ...c, query: q, sel: 0 } });
      return true;
    }
    if (c.step === "key") {
      if (k.name === "escape") return setPopup({ kind: "connect", c: { step: "provider", query: "", sel: 0 } }), true;
      if (k.name === "return") {
        const key = c.key.trim();
        if (!key) return true;
        try {
          const where = driver.storeKey(c.provider.id, key);
          say(`${c.provider.name} key saved to the ${where}`, "ok");
          void fetchForConnect(c.provider, true);
        } catch (e: any) {
          setPopup({ kind: "connect", c: { ...c, error: String(e?.message ?? e) } });
        }
        return true;
      }
      if (k.name === "backspace") return setPopup({ kind: "connect", c: { ...c, key: c.key.slice(0, -1) } }), true;
      if (k.name === "text" && k.text) setPopup({ kind: "connect", c: { ...c, key: c.key + k.text, error: undefined } });
      return true;
    }
    if (c.step === "wait") {
      if (k.name === "escape") {
        connectAbort.current?.abort();
        setPopup({ kind: "connect", c: { step: "provider", query: "", sel: 0 } });
      }
      return true;
    }
    // models
    const order = rank(c.models, c.query, (m) => `${m.id} ${m.name}`);
    if (k.name === "escape") return setPopup({ kind: "connect", c: { step: "provider", query: "", sel: 0 } }), true;
    const m = move(c.sel, order.length);
    if (m !== null) return setPopup({ kind: "connect", c: { ...c, sel: m } }), true;
    if (k.name === "return" && order.length) {
      const pick = c.models[order[Math.min(c.sel, order.length - 1)]];
      setPopup(null);
      void setModelTo(`${c.provider.id}/${pick.id}`);
      return true;
    }
    const q = typing(c.query);
    if (q !== null) setPopup({ kind: "connect", c: { ...c, query: q, sel: 0 } });
    return true;
  };

  const onKey = (k: Key) => {
    // ctrl+c: clear the input, else interrupt, else quit on a second press
    if (k.ctrl && k.name === "c") {
      if (popup) return setPopup(null);
      if (buf) return setInput("", 0);
      if (busyRef.current) return cancel();
      if (Date.now() - quitArmed < 2000) return quit();
      setQuitArmed(Date.now());
      return;
    }
    if (k.ctrl && k.name === "d" && !buf && !popup) return quit();
    if (k.name === "paste") return onPaste(k.text ?? "");

    if (perm) {
      const n = perm.options.length;
      const answer = (i: number) => {
        driver.reply(perm.id, { outcome: { outcome: "selected", optionId: perm.options[i].id } });
        setPerm(null);
      };
      if (k.name === "up" || k.name === "left") setPermSel((s) => (s - 1 + n) % n);
      else if (k.name === "down" || k.name === "right" || k.name === "tab") setPermSel((s) => (s + 1) % n);
      else if (k.name === "text" && /^[1-9]$/.test(k.text ?? "") && +k.text! <= n) answer(+k.text! - 1);
      else if (k.name === "text" && /^[yY]$/.test(k.text ?? "")) answer(0);
      else if (k.name === "text" && /^[nN]$/.test(k.text ?? "")) answer(n - 1);
      else if (k.name === "return") answer(permSel);
      else if (k.name === "escape") {
        answer(n - 1);
        cancel();
      }
      return;
    }
    if (onPopupKey(k)) return;

    if (k.name === "tab" && k.shift) {
      const next = MODES[(MODES.indexOf(mode) + 1) % MODES.length];
      return void setModeTo(next);
    }
    if (k.ctrl && k.name === "o") {
      setVerbose((v) => !v);
      return say(verbose ? "Compact results" : "Full results for tool output from here on (ctrl+o toggles)");
    }
    if (k.ctrl && (k.name === "space" || k.name === "`")) return dictate();
    if (k.ctrl && k.name === "l") {
      stdout.write("\x1b[2J\x1b[H");
      return setTr((s) => ({ ...s, key: s.key + 1 }));
    }
    if (k.name === "escape") {
      if (busyRef.current) return cancel();
      if (buf) {
        histIdx.current = -1;
        return setInput("", 0);
      }
      return;
    }

    // menus under the composer
    const menu = slashMenu ?? fileMenu?.list ?? null;
    if (menu && menu.length) {
      const n = Math.min(menu.length, 50);
      if (k.name === "up") return setMenuSel((s) => (s - 1 + n) % n);
      if (k.name === "down") return setMenuSel((s) => (s + 1) % n);
      if (k.name === "tab" || (k.name === "return" && fileMenu)) {
        if (slashMenu) {
          const c = slashMenu[Math.min(menuSel, slashMenu.length - 1)];
          return setInput(`/${c.name} `, c.name.length + 2);
        }
        const f = fileMenu!.list[Math.min(menuSel, fileMenu!.list.length - 1)];
        const nb = buf.slice(0, fileMenu!.at.start) + `@${f} ` + buf.slice(cur);
        return setInput(nb, fileMenu!.at.start + f.length + 2);
      }
      if (k.name === "return" && slashMenu) {
        const c = slashMenu[Math.min(menuSel, slashMenu.length - 1)];
        setInput("", 0);
        history.current.push(`/${c.name}`);
        appendHistory(`/${c.name}`);
        return void runSlash(c.name, "");
      }
    }

    // editing
    switch (k.name) {
      case "return":
        if (k.meta || k.shift) return insertText("\n");
        if (cur > 0 && buf[cur - 1] === "\\") return setInput(buf.slice(0, cur - 1) + "\n" + buf.slice(cur), cur);
        return submit();
      case "j":
        if (k.ctrl) return insertText("\n");
        break;
      case "backspace":
        if (k.meta || k.ctrl) {
          const w = wordLeft(buf, cur);
          return setInput(buf.slice(0, w) + buf.slice(cur), w);
        }
        if (cur > 0) setInput(buf.slice(0, cur - 1) + buf.slice(cur), cur - 1);
        return;
      case "delete":
        if (cur < buf.length) setInput(buf.slice(0, cur) + buf.slice(cur + 1), cur);
        return;
      case "left":
        return setCur(k.ctrl || k.meta ? wordLeft(buf, cur) : Math.max(0, cur - 1));
      case "right":
        return setCur(k.ctrl || k.meta ? wordRight(buf, cur) : Math.min(buf.length, cur + 1));
      case "home":
      case "a":
        if (k.name === "a" && !k.ctrl) break;
        return setCur(lineStart(buf, cur));
      case "end":
      case "e":
        if (k.name === "e" && !k.ctrl) break;
        return setCur(lineEnd(buf, cur));
      case "u":
        if (!k.ctrl) break;
        return setInput(buf.slice(0, lineStart(buf, cur)) + buf.slice(cur), lineStart(buf, cur));
      case "k":
        if (!k.ctrl) break;
        return setInput(buf.slice(0, cur) + buf.slice(lineEnd(buf, cur)), cur);
      case "w":
        if (!k.ctrl) break;
        return setInput(buf.slice(0, wordLeft(buf, cur)) + buf.slice(cur), wordLeft(buf, cur));
      case "up": {
        const v = verticalMove(buf, cur, -1);
        if (v !== null) return setCur(v);
        const h = history.current;
        if (!h.length) return;
        if (histIdx.current === -1) {
          draft.current = buf;
          histIdx.current = h.length - 1;
        } else if (histIdx.current > 0) histIdx.current--;
        return setInput(h[histIdx.current], h[histIdx.current].length);
      }
      case "down": {
        const v = verticalMove(buf, cur, 1);
        if (v !== null) return setCur(v);
        if (histIdx.current === -1) return;
        const h = history.current;
        if (histIdx.current < h.length - 1) {
          histIdx.current++;
          return setInput(h[histIdx.current], h[histIdx.current].length);
        }
        histIdx.current = -1;
        return setInput(draft.current, draft.current.length);
      }
      case "text":
        if (k.text && !k.ctrl) {
          // a lone "?" on an empty prompt opens help, like Claude Code
          if (k.text === "?" && !buf) return setPopup({ kind: "help", scroll: 0 });
          return insertText(k.text);
        }
        return;
    }
  };

  // raw input → keys (see keys.ts for why not useInput)
  const onKeyRef = useRef(onKey);
  onKeyRef.current = onKey;
  useEffect(() => {
    if (!isRawModeSupported) return;
    setRawMode(true);
    stdout.write("\x1b[?2004h"); // bracketed paste
    const parser = new KeyParser();
    const onData = (chunk: string | Buffer) => {
      for (const k of parser.feed(String(chunk))) onKeyRef.current(k);
    };
    stdinEvents.on("input", onData);
    return () => {
      stdinEvents.removeListener("input", onData);
      stdout.write("\x1b[?2004l");
      setRawMode(false);
    };
  }, [isRawModeSupported, setRawMode, stdinEvents]);

  // ── render ────────────────────────────────────────────────────────────
  const reads = useMemo<ReadCache>(() => {
    const m: ReadCache = new Map();
    for (const it of [...tr.done, ...tr.live]) {
      if (it.kind !== "tool" || it.name !== "read" || !it.detail) continue;
      const p = String(it.input?.path ?? it.arg ?? "");
      if (!p) continue;
      const cur2 = m.get(p) ?? new Map<number, string>();
      for (const [n, text] of parseReadDetail(it.detail)) cur2.set(n, text);
      m.set(p, cur2);
    }
    return m;
  }, [tr.done, tr.live]);

  const header = (
    <HeaderBox version="0.1.0" user={user} model={model} cwd={boot.cwd} width={Math.min(width, 110)} />
  );
  const renderItem = (item: Item) => <TranscriptItem key={item.id} item={item} verbose={verbose} reads={reads} header={header} />;
  const rowsNow = stdout?.rows ?? 30;
  // the growing reply may only use what the terminal has left under it
  const liveCap = { rows: Math.max(4, rowsNow - 14), width: width - 4 };
  const renderLive = (item: Item, i: number, arr: Item[]) =>
    item.kind === "assistant" && busy && i === arr.length - 1 ? (
      <TranscriptItem key={item.id} item={item} verbose={verbose} reads={reads} cap={liveCap} />
    ) : (
      renderItem(item)
    );
  const used = tr.usage.input + tr.usage.cache_read + tr.usage.cache_write;
  const rows = stdout?.rows ?? 30;
  const menuH = Math.max(4, Math.min(10, rows - 14));

  let overlay: React.ReactNode = null;
  if (popup?.kind === "help") {
    const keys: Row[] = [
      ["enter", "send · \\+enter or ctrl+j for a new line"],
      ["esc", "interrupt the agent · close menus"],
      ["shift+tab", "cycle plan → agent → accept-edits → yolo"],
      ["↑ ↓", "prompt history · move between lines"],
      ["@", "attach a file (fuzzy search)"],
      ["!cmd", "run a shell command; its output rides with your next message"],
      ["#text", "remember something in the shared memory vault"],
      ["ctrl+o", "full / compact tool results"],
      ["ctrl+space", "dictate"],
      ["ctrl+l", "redraw the screen"],
      ["ctrl+c", "clear input · interrupt · twice to quit"],
    ].map(([a, b]) => ({ key: a, label: a, detail: b }));
    const cmds: Row[] = commands.map((c) => ({ key: `/${c.name}`, label: `/${c.name}${c.hint ? ` ${c.hint}` : ""}`, detail: c.desc }));
    const h = Math.max(6, rows - 15);
    const all = [...keys, ...cmds];
    overlay = (
      <Picker
        title="zdev — keys and commands"
        rows={all}
        sel={-1}
        scroll={Math.min(popup.scroll, Math.max(0, all.length - h))}
        height={h}
        width={width}
        footer="↑↓ scroll · any other key closes"
      />
    );
  } else if (popup?.kind === "resume") {
    overlay = (
      <Picker
        title="Resume a session"
        rows={popup.sessions.map((s) => ({
          key: s.id,
          label: String(s.started ?? "").slice(5, 16).replace("T", " "),
          detail: String(s.preview ?? "").replace(/\s+/g, " ").slice(0, width - 30),
          right: s.harness,
        }))}
        sel={popup.sel}
        height={menuH}
        width={width}
        footer="↑↓ choose · enter resume · esc cancel"
      />
    );
  } else if (popup?.kind === "model") {
    const order = rank(popup.models, popup.query, (m) => `${m.alias} ${m.name} ${m.providerName}`);
    const curAlias = model.split(" ")[0];
    let lastProv = "";
    const list: Row[] = order.map((i) => {
      const m = popup.models[i];
      const section = !popup.query && m.providerName !== lastProv ? m.providerName : undefined;
      lastProv = m.providerName;
      return {
        key: m.alias,
        label: m.fromConfig ? m.alias : m.id,
        detail: m.fromConfig ? ` ${m.id}` : m.name !== m.id ? ` ${m.name}` : undefined,
        tag: m.alias === curAlias ? "● current" : undefined,
        right: `${m.context ? fmtK(m.context) : ""}  ${priceText(m)}`,
        section,
      };
    });
    const sel = popup.models[order[Math.min(popup.sel, order.length - 1)]];
    overlay = (
      <Picker
        title="Model"
        subtitle={
          <Text color={DIM}>
            {sel?.reasoning || popup.effort ? (
              <>
                effort <Text color={ROSE}>{"← "}{popup.effort || "default"}{" →"}</Text> ·{" "}
              </>
            ) : null}
            {popup.models.length} models from your connected providers · /connect adds more
          </Text>
        }
        query={popup.query}
        rows={list}
        sel={popup.sel}
        height={menuH}
        width={width}
        loading={popup.loading}
        footer="↑↓ choose · enter use & save as default · ←→ effort · ctrl+r refresh live list · esc"
      />
    );
  } else if (popup?.kind === "connect") {
    const c = popup.c;
    if (c.step === "provider") {
      overlay = (
        <Picker
          title="Providers"
          subtitle="Keys stay on this machine (Windows Credential Manager) and work in zcli too."
          query={c.query}
          placeholder="Type to search providers"
          rows={providerRows(c.query).map((x) => ({
            key: x.def.id,
            label: x.def.kind === "chatgpt" ? "ChatGPT" : x.def.kind === "devin" ? "Devin" : x.def.name,
            detail: ` ${pitchFor(x.def)}`,
            tag: x.ready ? `✓ ${x.detail}` : undefined,
          }))}
          sel={c.sel}
          height={menuH}
          width={width}
          footer="↑↓ choose · enter connect (connected → pick a model) · esc"
        />
      );
    } else if (c.step === "key") {
      const pin = PINNED.find((x) => x.id === c.provider.id);
      overlay = (
        <Box flexDirection="column" borderStyle="round" borderColor="#5a5a5a" paddingX={1} marginTop={1} width={width}>
          <Text bold color={ROSE}>
            {c.provider.name} API key
          </Text>
          <Text color={DIM}>Paste or type it — hidden as you type. It's checked against the provider before it's used.</Text>
          <Box marginTop={1}>
            <Text>
              <Text color={ROSE}>❯ </Text>
              {"•".repeat(Math.min(c.key.length, width - 8))}
              <Text inverse> </Text>
            </Text>
          </Box>
          {c.error ? <Text color="#d0d0d0">✗ {c.error}</Text> : null}
          {pin?.url ? (
            <Text color={DIM}>
              get one at <Text color={GRAY}>{pin.url}</Text>
            </Text>
          ) : null}
          {c.provider.env[0] ? <Text color={DIM}>or set {c.provider.env[0]} in your environment</Text> : null}
          <Text color={DIM}>enter save · esc back</Text>
        </Box>
      );
    } else if (c.step === "wait") {
      overlay = (
        <Box flexDirection="column" borderStyle="round" borderColor="#5a5a5a" paddingX={1} marginTop={1} width={width}>
          <Thinking running verb={c.message} />
          {c.url ? (
            <>
              <Text color={DIM}>If the browser didn't open, visit:</Text>
              <Text color={GRAY} wrap="truncate-middle">
                {c.url}
              </Text>
            </>
          ) : null}
          <Text color={DIM}>esc cancel</Text>
        </Box>
      );
    } else {
      const order = rank(c.models, c.query, (m) => `${m.id} ${m.name}`);
      overlay = (
        <Picker
          title={`Choose a model — ${c.provider.kind === "chatgpt" ? "ChatGPT" : c.provider.name}`}
          subtitle={c.source}
          query={c.query}
          rows={order.map((i) => {
            const m = c.models[i];
            return {
              key: m.id,
              label: m.id,
              detail: m.name !== m.id ? ` ${m.name}` : undefined,
              right: `${fmtK(m.context)}  ${c.provider.kind === "chatgpt" || c.provider.kind === "devin" ? "plan" : m.price[0] || m.price[1] ? `${money(m.price[0])}/${money(m.price[1])}` : c.provider.id === "openrouter" ? "free" : ""}`,
            };
          })}
          sel={c.sel}
          height={menuH}
          width={width}
          footer="↑↓ choose · enter use & save as default · esc back"
        />
      );
    }
  }

  let menu: React.ReactNode = null;
  if (slashMenu && slashMenu.length) {
    menu = (
      <Picker
        boxed={false}
        rows={slashMenu.map((c) => ({ key: c.name, label: `/${c.name}`, detail: `${c.hint ? `${c.hint}  ` : ""}${c.desc}` }))}
        sel={menuSel}
        height={Math.min(8, menuH)}
        width={width}
      />
    );
  } else if (fileMenu && fileMenu.list.length) {
    menu = <Picker boxed={false} rows={fileMenu.list.map((f) => ({ key: f, label: f }))} sel={menuSel} height={Math.min(8, menuH)} width={width} />;
  }

  const hint = quitArmed && Date.now() - quitArmed < 2000 ? "press ctrl+c again to quit" : undefined;

  return (
    <Box flexDirection="column">
      <Static key={tr.key} items={tr.done}>
        {renderItem}
      </Static>
      {tr.live.map(renderLive)}
      {busy && !perm ? (
        <Box marginTop={1}>
          <Thinking running tokens={tr.usage.output || undefined} />
        </Box>
      ) : null}
      {perm ? <Permission req={perm} sel={permSel} reads={reads} /> : null}
      {overlay}
      {flash ? (
        <Box marginTop={1}>
          <Text color={flash.tone === "error" ? "#d6d6d6" : flash.tone === "ok" ? GREEN : GRAY} wrap="truncate-end">
            {flash.tone === "error" ? "✗ " : flash.tone === "ok" ? "✓ " : "· "}
            {flash.text}
          </Text>
        </Box>
      ) : null}
      <Composer
        value={buf}
        cursor={cur}
        mode={mode}
        model={model}
        width={width}
        disabled={!sessionId}
        listening={listening}
        ctxPct={ctxLimit ? used / ctxLimit : undefined}
        hint={hint}
      />
      {menu}
    </Box>
  );
}
