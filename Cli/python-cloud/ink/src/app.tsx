// App — wires AcpClient → transcript reducer, routes keys by focus
// (permission > model picker > transcript-nav > composer).
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Box, Static, useApp, useInput } from "ink";
import { AcpClient } from "./acp";
import { applyUpdate, filterSlash, SLASH_COMMANDS } from "./reducer";
import { emptyState } from "./types";
import type { Item, Model, PermissionReq, State } from "./types";
import { Header } from "./components/Header";
import { TranscriptItem } from "./components/Transcript";
import { Thinking } from "./components/Thinking";
import { Composer } from "./components/Composer";
import { SlashMenu } from "./components/SlashMenu";
import { Permission } from "./components/Permission";
import { ModelPicker } from "./components/ModelPicker";
import { Footer } from "./components/Footer";
import { DIM } from "./theme";

let nid = 0;
const nextId = () => `u${++nid}`;
const note = (text: string): Item => ({ kind: "note", id: nextId(), text });
const asst = (text: string): Item => ({ kind: "assistant", id: nextId(), text });

export type Boot = { cwd: string; model?: string; mode?: string };

export function App({ boot }: { boot: Boot }) {
  const { exit } = useApp();
  const [state, setState] = useState<State>(emptyState());
  const [sessionId, setSessionId] = useState("");
  const [version, setVersion] = useState("0.1.0");
  const [mode, setMode] = useState(boot.mode ?? "agent");
  const [model, setModel] = useState(boot.model ?? "…");
  const [busy, setBusy] = useState(false);
  const [buf, setBuf] = useState("");
  const [cur, setCur] = useState(0);
  const [slashSel, setSlashSel] = useState(0);
  const [perm, setPerm] = useState<PermissionReq | null>(null);
  const [permSel, setPermSel] = useState(0);
  const [picker, setPicker] = useState<{
    open: boolean;
    loading: boolean;
    models: Model[];
    sel: number;
  }>({ open: false, loading: false, models: [], sel: 0 });
  const [nav, setNav] = useState(false); // transcript-nav focus mode
  const [navIdx, setNavIdx] = useState(0);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [status, setStatus] = useState("connecting…");

  const clientRef = useRef<AcpClient | null>(null);
  const dropRef = useRef(false); // ignore updates after esc-cancel
  const histRef = useRef<string[]>([]);
  const histIdxRef = useRef(-1);
  const draftRef = useRef("");

  const pushItems = useCallback((...items: Item[]) => {
    setState((s) => ({ ...s, items: [...s.items, ...items] }));
  }, []);

  // ── boot: spawn → initialize → session/new ──────────────────────────────
  useEffect(() => {
    const client = new AcpClient(boot.cwd);
    clientRef.current = client;
    client.onUpdate = (_sid, u) => {
      if (dropRef.current) return;
      setState((s) => applyUpdate(s, u));
    };
    client.onRequest = (req) => {
      if (req.method === "session/request_permission") {
        setPermSel(0);
        setPerm({
          id: req.id,
          toolCallId: req.params?.toolCallId ?? "",
          tool: req.params?.tool ?? req.params?.toolCallId ?? "tool",
          input: req.params?.input ?? {},
          options: req.params?.options ?? [
            { id: "allow", name: "Allow once" },
            { id: "allow_always", name: "Always allow" },
            { id: "deny", name: "Deny" },
          ],
        });
      } else {
        client.reply(req.id, undefined); // unknown server→client call: empty
      }
    };
    client.onExit = (code) => {
      setStatus(`acp exited (${code})`);
      pushItems(note(`backend exited (code ${code})`));
      if (client.stderrTail.length)
        pushItems(note(client.stderrTail[client.stderrTail.length - 1].slice(0, 200)));
    };
    (async () => {
      try {
        const init: any = await client.call("initialize", {});
        const v = init.result?.agentInfo?.version;
        if (v) setVersion(v);
        const sn: any = await client.call("session/new", {
          cwd: boot.cwd,
          ...(boot.model ? { model: boot.model } : {}),
          ...(boot.mode ? { mode: boot.mode } : {}),
        });
        if (sn.error) throw new Error(sn.error.message);
        setSessionId(sn.result.sessionId);
        setModel(sn.result.model ?? boot.model ?? "?");
        setMode(sn.result.mode ?? boot.mode ?? "agent");
        setStatus("");
      } catch (e: any) {
        setStatus("session failed");
        pushItems(note(`session/new failed: ${e?.message ?? e}`));
      }
    })();
    return () => client.kill();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const quit = useCallback(() => {
    clientRef.current?.kill();
    exit();
    setTimeout(() => process.exit(0), 50);
  }, [exit]);

  // ── prompt / approve ────────────────────────────────────────────────────
  const runPrompt = useCallback(
    async (method: "session/prompt" | "session/approve", text?: string) => {
      const client = clientRef.current;
      if (!client || !sessionId || busy) return;
      setBusy(true);
      dropRef.current = false;
      try {
        const params =
          method === "session/prompt"
            ? { sessionId, prompt: [{ type: "text", text }] }
            : { sessionId };
        const res: any = await client.call(method, params);
        const reason = res?.result?.stopReason;
        if (res?.error) pushItems(note(`error: ${res.error.message}`));
        else if (reason === "cancelled") pushItems(note("interrupted (esc)"));
        else if (reason === "refusal")
          pushItems(note("turn failed — provider error (see zdev stderr)"));
        else if (reason === "max_turns") pushItems(note("stopped: max turns"));
      } catch (e: any) {
        pushItems(note(`backend: ${e?.message ?? e}`));
      } finally {
        dropRef.current = false;
        setBusy(false);
      }
    },
    [busy, pushItems, sessionId],
  );

  const sendPrompt = useCallback(
    (text: string) => {
      pushItems({ kind: "user", id: nextId(), text });
      histRef.current.push(text);
      histIdxRef.current = -1;
      void runPrompt("session/prompt", text);
    },
    [pushItems, runPrompt],
  );

  const cancel = useCallback(() => {
    if (!busy) return;
    dropRef.current = true;
    clientRef.current?.notify("session/cancel", { sessionId });
    pushItems(note("interrupt requested…"));
  }, [busy, pushItems, sessionId]);

  // ── slash commands ──────────────────────────────────────────────────────
  const slashOpen = /^\/\S*$/.test(buf);
  const slashQuery = slashOpen ? buf.slice(1) : "";
  const slashList = useMemo(
    () => (slashOpen ? filterSlash(slashQuery) : []),
    [slashOpen, slashQuery],
  );

  const openPicker = useCallback(async () => {
    const client = clientRef.current;
    setPicker({ open: true, loading: true, models: [], sel: 0 });
    try {
      const res: any = await client!.call("session/list_models", {});
      const models: Model[] = res.result?.models ?? [];
      const curAlias = model.split(" ")[0];
      const sel = Math.max(0, models.findIndex((m) => m.alias === curAlias));
      setPicker({ open: true, loading: false, models, sel });
    } catch (e: any) {
      setPicker({ open: false, loading: false, models: [], sel: 0 });
      pushItems(note(`list_models failed: ${e?.message ?? e}`));
    }
  }, [model, pushItems]);

  const runSlash = useCallback(
    async (cmd: string) => {
      const client = clientRef.current;
      const name = cmd.split(/\s/)[0];
      switch (name) {
        case "/plan":
        case "/agent":
        case "/yolo": {
          const m = name.slice(1);
          const res: any = await client?.call("session/set_mode", {
            sessionId,
            mode: m,
          });
          if (res?.error) pushItems(note(`set_mode: ${res.error.message}`));
          else {
            setMode(m);
            pushItems(note(`mode: → ${m}`));
          }
          break;
        }
        case "/approve":
          void runPrompt("session/approve");
          break;
        case "/model":
          void openPicker();
          break;
        case "/session":
          pushItems(note(`session ${sessionId}`));
          break;
        case "/usage":
          pushItems(
            asst(
              `**usage** — in ${state.usage.input} · out ${state.usage.output}` +
                ` · cache r/w ${state.usage.cache_read}/${state.usage.cache_write}` +
                (state.usage.cum_usd != null
                  ? ` · Σ $${state.usage.cum_usd.toFixed(4)}`
                  : ""),
            ),
          );
          break;
        case "/help":
          pushItems(
            asst(
              [
                "**commands**",
                ...SLASH_COMMANDS.map((c) => `- \`${c.name}\` — ${c.desc}`),
                "",
                "**keys** — enter send · shift/alt+enter newline · ↑/↓ history",
                "ctrl+o transcript select → enter/o expands · esc interrupt · ctrl+c quit",
              ].join("\n"),
            ),
          );
          break;
        case "/clear":
          setState((s) => ({ ...s, items: [] }));
          break;
        case "/quit":
          quit();
          break;
        default:
          sendPrompt(cmd);
      }
    },
    [model, openPicker, pushItems, quit, runPrompt, sendPrompt, sessionId, state.usage],
  );

  const submit = useCallback(
    (override?: string) => {
      const text = override ?? buf;
      if (!text.trim() || !sessionId || busy) return;
      // recompute from `text` — `buf`/`slashList` can be stale on atomic input
      const list = /^\/\S*$/.test(text) ? filterSlash(text.slice(1)) : [];
      if (list.length) {
        const hit = list[Math.min(slashSel, list.length - 1)];
        if (text === "/") {
          // "/" alone completes to the highlighted command
          setBuf(hit.name);
          setCur(hit.name.length);
          return;
        }
        const exact = list.find((c) => c.name === text);
        setBuf("");
        setCur(0);
        setSlashSel(0);
        void runSlash((exact ?? hit).name);
        return;
      }
      setBuf("");
      setCur(0);
      setSlashSel(0);
      if (text.startsWith("/")) void runSlash(text); // unknown /cmd → sent as prompt
      else sendPrompt(text);
    },
    [buf, busy, runSlash, sendPrompt, sessionId, slashSel],
  );

  // ── transcript expand/nav ───────────────────────────────────────────────
  const toolItems = useMemo(
    () => state.items.filter((i): i is Extract<Item, { kind: "tool" }> => i.kind === "tool"),
    [state.items],
  );
  const navTarget = toolItems[Math.min(navIdx, toolItems.length - 1)];

  const toggleExpand = useCallback(
    (toolCallId: string) =>
      setExpanded((e) => ({ ...e, [toolCallId]: !e[toolCallId] })),
    [],
  );

  // ── key routing ─────────────────────────────────────────────────────────
  const onKey = useCallback(
    (input: string, key: any) => {
      if (key.ctrl && (input === "c" || input === "\x03")) return quit();
      if (key.ctrl && input === "d" && !buf) return quit();

      // permission radiogroup
      if (perm) {
        const n = perm.options.length;
        const digit = /^[1-9][\r\n]*$/.test(input) ? +input.trim()[0] : 0;
        if (key.downArrow || key.rightArrow) setPermSel((s) => (s + 1) % n);
        else if (key.upArrow || key.leftArrow)
          setPermSel((s) => (s - 1 + n) % n);
        else if (digit) {
          if (digit <= n) answerPerm(digit - 1);
        } else if (key.return || /^[\r\n]+$/.test(input)) answerPerm(permSel);
        else if (key.escape) {
          answerPerm(n - 1); // esc = deny
          cancel();          // …and interrupt the turn, like Claude Code
        }
        return;
        function answerPerm(i: number) {
          const opt = perm!.options[i];
          clientRef.current?.reply(perm!.id, {
            outcome: { outcome: "selected", optionId: opt.id },
          });
          setPerm(null);
        }
      }

      // model picker
      if (picker.open) {
        const n = picker.models.length;
        if (key.escape) setPicker((p) => ({ ...p, open: false }));
        else if (key.downArrow) setPicker((p) => ({ ...p, sel: (p.sel + 1) % Math.max(1, n) }));
        else if (key.upArrow) setPicker((p) => ({ ...p, sel: (p.sel - 1 + n) % Math.max(1, n) }));
        else if ((key.return || /^[\r\n]+$/.test(input)) && n) {
          const m = picker.models[picker.sel];
          setPicker((p) => ({ ...p, open: false }));
          void (async () => {
            const res: any = await clientRef.current?.call("session/set_model", {
              sessionId,
              model: m.alias,
            });
            if (res?.error) pushItems(note(`set_model: ${res.error.message}`));
            else {
              setModel(res.result.model);
              pushItems(note(`model: ${res.result.model}`));
            }
          })();
        }
        return;
      }

      // transcript-nav mode: ↑/↓ pick a tool call, enter/o toggles detail
      if (nav) {
        const n = toolItems.length;
        if (key.escape || (key.ctrl && input === "o")) {
          setNav(false);
          setStatus("");
        } else if (key.downArrow) setNavIdx((i) => Math.min(i + 1, n - 1));
        else if (key.upArrow) setNavIdx((i) => Math.max(i - 1, 0));
        else if (
          key.return ||
          /^[\r\n]+$/.test(input) ||
          input.replace(/[\r\n]+$/, "") === "o"
        ) {
          if (navTarget) toggleExpand(navTarget.toolCallId);
        }
        return;
      }
      if (key.ctrl && input === "o") {
        if (toolItems.length) {
          setNav(true);
          setNavIdx(toolItems.length - 1);
          setStatus("transcript — ↑/↓ select · enter/o expand · esc back");
        }
        return;
      }

      // composer
      if (key.escape) {
        if (busy) return cancel();
        if (buf) {
          setBuf("");
          setCur(0);
        }
        return;
      }
      if (slashOpen && slashList.length) {
        if (key.downArrow) return setSlashSel((s) => (s + 1) % slashList.length);
        if (key.upArrow)
          return setSlashSel((s) => (s - 1 + slashList.length) % slashList.length);
        if (key.tab) {
          const c = slashList[Math.min(slashSel, slashList.length - 1)];
          setBuf(c.name);
          setCur(c.name.length);
          return;
        }
      }
      // Enter may arrive as \r, \n or \r\n (winpty sends the pair as one
      // keypress); treat any pure CR/LF input as return, not literal text.
      if (key.return || /^[\r\n]+$/.test(input)) {
        if (key.shift || key.meta) return insert("\n");
        return submit();
      }
      if (key.tab) return insert("  ");
      if (key.backspace || input === "\x7f") {
        if (cur > 0) {
          setBuf((b) => b.slice(0, cur - 1) + b.slice(cur));
          setCur(cur - 1);
        }
        return;
      }
      if (key.delete) {
        setBuf((b) => b.slice(0, cur) + b.slice(cur + 1));
        return;
      }
      if (key.leftArrow) return setCur((c) => Math.max(0, c - 1));
      if (key.rightArrow) return setCur((c) => Math.min(buf.length, c + 1));
      if (key.home) return setCur(0);
      if (key.end) return setCur(buf.length);
      if (key.upArrow) {
        // history when cursor sits on the first line
        const h = histRef.current;
        if (!h.length || buf.includes("\n")) return;
        if (histIdxRef.current === -1) {
          draftRef.current = buf;
          histIdxRef.current = h.length - 1;
        } else if (histIdxRef.current > 0) histIdxRef.current--;
        setBuf(h[histIdxRef.current]);
        setCur(h[histIdxRef.current].length);
        return;
      }
      if (key.downArrow) {
        const h = histRef.current;
        if (histIdxRef.current === -1) return;
        if (histIdxRef.current < h.length - 1) {
          histIdxRef.current++;
          setBuf(h[histIdxRef.current]);
          setCur(h[histIdxRef.current].length);
        } else {
          histIdxRef.current = -1;
          setBuf(draftRef.current);
          setCur(draftRef.current.length);
        }
        return;
      }
      if (input && !key.ctrl && !key.meta) {
        // A chunk may carry a trailing newline (one write = one keypress, or a
        // paste ending in ⏎): insert the text and submit what remains.
        const tail = input.match(/[\r\n]+$/);
        if (tail) {
          const typed = input.slice(0, input.length - tail[0].length);
          const nb = buf.slice(0, cur) + typed.replace(/\r\n?/g, "\n") + buf.slice(cur);
          setBuf(nb);
          setCur(cur + typed.length);
          return submit(nb);
        }
        return insert(input);
      }

      function insert(s: string) {
        const text = s.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
        setBuf((b) => b.slice(0, cur) + text + b.slice(cur));
        setCur(cur + text.length);
        setSlashSel(0);
      }
    },
    [buf, busy, cancel, cur, nav, navTarget, perm, picker, pushItems, quit, sessionId, slashList, slashOpen, submit, toggleExpand, toolItems],
  );

  // input is only meaningful on a TTY; keep rendering headless for smoke runs
  const keyRouter = process.stdin.isTTY ? <KeyRouter onKey={onKey} /> : null;

  const user = process.env.USERNAME ?? process.env.USER ?? "operator";

  // <Static> renders each item once — mutated rows (pending tool → done,
  // expand toggles) must live in the normal flow until they settle.
  // Boundary: the last user item. Its whole turn (assistant text, tool rows,
  // notes) stays live + expandable; everything older commits to scrollback.
  let liveStart = state.items.length;
  for (let i = state.items.length - 1; i >= 0; i--) {
    if (state.items[i].kind === "user") {
      liveStart = i;
      break;
    }
    const it = state.items[i];
    if (it.kind === "tool" && it.status === "pending") liveStart = i; // safety
  }
  const doneItems = state.items.slice(0, liveStart);
  const liveItems = state.items.slice(liveStart);
  const renderItem = (item: Item) => (
    <TranscriptItem
      key={item.id}
      item={item}
      expanded={!!expanded[item.kind === "tool" ? item.toolCallId : item.id]}
      focused={nav && item === navTarget}
    />
  );

  return (
    <Box flexDirection="column">
      {keyRouter}
      <Header
        version={version}
        user={user}
        model={model}
        cwd={boot.cwd}
        width={(process.stdout.columns ?? 80) - 2}
      />
      <Static items={doneItems}>{renderItem}</Static>
      {liveItems.map(renderItem)}
      <Thinking running={busy && !perm} tokens={state.usage.output || undefined} />
      {perm ? <Permission req={perm} sel={permSel} /> : null}
      {picker.open ? (
        <ModelPicker
          models={picker.models}
          sel={picker.sel}
          current={model.split(" ")[0]}
          loading={picker.loading}
        />
      ) : null}
      {slashOpen && !busy ? <SlashMenu list={slashList} active={slashSel} /> : null}
      <Composer
        value={buf}
        cursor={cur}
        mode={mode}
        model={model}
        disabled={!sessionId}
      />
      <Footer usage={state.usage} sessionId={sessionId} status={status} />
    </Box>
  );
}

function KeyRouter({ onKey }: { onKey: (i: string, k: any) => void }) {
  useInput(onKey);
  return null;
}
