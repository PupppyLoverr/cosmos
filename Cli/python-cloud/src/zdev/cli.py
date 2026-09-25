"""zdev — Cosmos Python harness. `zdev -p "task"` headless, bare `zdev` REPL."""

from __future__ import annotations

import itertools
import json
import os
import queue
import shutil
import subprocess
import sys
import threading
from pathlib import Path

from . import agent, proto, store, tools as t

HELP = """\
zdev — Cosmos coding-agent harness (Python)

usage:
  zdev                        interactive session (REPL)
  zdev -p "<task>" [flags]    headless run; -p - reads the task from stdin
  zdev ui                     launch the Ink TUI (python-cloud/ink, needs bun)
  zdev acp                    ACP stdio JSON-RPC server (used by `zdev ui`)
  zdev tools dump             canonical tools.json (byte-identical to zcli/zagy)
  zdev session list           sessions in COSMOS_CLI_HOME
  zdev session export <id> [--format md|jsonl]
  zdev session branch <id> --at <event_id>
  zdev session tree <id>
  zdev memory search <q>      search the shared Meridian vault
  zdev memory add <text>      append to the vault (scope: zdev)

flags: --output text|json|json-stream · --mode plan|agent · --allow a,b ·
--yolo · --no-sandbox · --model <alias> · --max-turns N · --max-usd F ·
--cwd <dir> · --resume <id> · --approve-plan · --allow-escalation cap,...
"""


def _parse(args: list[str]) -> proto.RunOpts:
    o = proto.RunOpts()
    positional: list[str] = []
    i = 0
    while i < len(args):
        a = args[i]

        def val() -> str:
            nonlocal i
            i += 1
            if i >= len(args):
                raise ValueError(f"{a} needs a value")
            return args[i]

        if a in ("-p", "--prompt"):
            o.prompt = val()
        elif a == "--output":
            o.output = val()
            if o.output not in ("text", "json", "json-stream"):
                raise ValueError(f"bad --output {o.output}")
        elif a == "--mode":
            o.mode = val()
        elif a == "--allow":
            o.allow = [x.strip() for x in val().split(",") if x.strip()]
        elif a == "--yolo":
            o.yolo = True
        elif a == "--no-sandbox":
            o.no_sandbox = True
        elif a == "--model":
            o.model = val()
        elif a == "--max-turns":
            o.max_turns = int(val())
        elif a == "--max-usd":
            o.max_usd = float(val())
        elif a == "--cwd":
            o.cwd = val()
        elif a == "--resume":
            o.resume = val()
        elif a == "--approve-plan":
            o.approve_plan = True
        elif a == "--allow-escalation":
            o.allow_escalation = [x.strip() for x in val().split(",") if x.strip()]
        elif a in ("--version", "--help", "-h", "-V"):
            print(f"zdev {proto.CONTRACT_VERSION} (contract v1)\n{HELP}")
            raise SystemExit(proto.EXIT_DONE)
        elif a.startswith("-"):
            raise ValueError(f"unknown flag {a}")
        else:
            positional.append(a)
        i += 1
    if o.prompt is None and positional:
        o.prompt = " ".join(positional)
    return o


def _render(ev: dict) -> None:
    k, d = ev["kind"], ev.get("data", {})
    if k == "assistant":
        if d.get("text"):
            print(d["text"])
        for c in d.get("tool_calls") or []:
            print(f"  → {c.get('name')} {json.dumps(c.get('input'))[:120]}",
                  file=sys.stderr)
    elif k == "tool_result":
        mark = "✗" if d.get("is_error") else "✓"
        print(f"  {mark} {d.get('name')}: {str(d.get('content'))[:160]}",
              file=sys.stderr)
    elif k == "plan":
        print(f"\n── plan ──\n{d.get('plan', '')}\n──────────\n"
              "(/approve to run, or keep refining)")
    elif k == "subagent_start":
        print(f"  ⎇ subagent: {d.get('prompt', '')}", file=sys.stderr)


def _run_turns(ag: agent.Agent) -> None:
    for _ in range(ag.opts.max_turns or 32):
        try:
            done = ag.turn(_render)
        except Exception as e:
            print(f"turn failed: {e}", file=sys.stderr)
            break
        if done:
            break
        calls = [(b["id"], b["name"], b["input"])
                 for b in ag.messages[-1]["content"] if b["type"] == "tool_use"]
        for tid, name, inp in calls:
            ag.exec_tool(tid, name, inp, _render)
            if name == "submit_plan":
                return  # wait for /approve


def repl(opts: proto.RunOpts) -> int:
    ag = agent.Agent(opts, "zdev", os.getcwd())
    print(f"zdev · {ag.model_alias} · mode {ag.mode} · session {ag.session.id[:8]}",
          file=sys.stderr)
    if ag.ctx.sandbox_degraded:
        print(f"sandbox degraded: {ag.ctx.sandbox_degraded}", file=sys.stderr)

    def asker(tool: str) -> bool:
        try:
            return input(f"allow {tool}? [y/N] ").strip().lower() in ("y", "yes")
        except EOFError:
            return False

    ag.asker = asker
    _render(ag.session.append("session_start", {
        "model": ag.model_alias, "mode": ag.mode,
        "sandbox": "off" if ag.ctx.no_sandbox else "appcontainer",
        "boot_tokens": ag.boot_tokens}))
    if opts.prompt and opts.prompt != "-":
        ag.send_user(opts.prompt, _render)
        _run_turns(ag)
    while True:
        try:
            line = input("› ").strip()
        except (EOFError, KeyboardInterrupt):
            break
        if not line:
            continue
        if line in ("/quit", "/exit", "/q"):
            break
        elif line == "/plan":
            ag.mode = "plan"; print("mode: plan", file=sys.stderr)
        elif line == "/agent":
            ag.mode = "agent"; print("mode: agent", file=sys.stderr)
        elif line == "/yolo":
            ag.mode = "yolo"; print("mode: yolo", file=sys.stderr)
        elif line == "/approve":
            ag.approve_plan(_render); _run_turns(ag)
        elif line == "/session":
            print(f"session {ag.session.id}", file=sys.stderr)
        elif line == "/help":
            print("commands: /plan /agent /yolo /approve /session /quit",
                  file=sys.stderr)
        else:
            ag.send_user(line, _render)
            _run_turns(ag)
    ag.session.append("session_end", {"reason": "quit"})
    return proto.EXIT_DONE


def _session_cmd(args: list[str]) -> int:
    home = proto.cli_home()
    sub = args[0] if args else ""
    if sub == "list":
        for s in store.Session.list(home):
            print(json.dumps(s))
        return proto.EXIT_DONE
    if sub == "export":
        sid = args[1] if len(args) > 1 else ""
        fmt = "md"
        if "--format" in args:
            i = args.index("--format")
            fmt = args[i + 1] if i + 1 < len(args) else "md"
        try:
            print(store.Session.export(home, sid, fmt), end="")
            return proto.EXIT_DONE
        except OSError as e:
            print(e, file=sys.stderr)
            return proto.EXIT_USAGE
    if sub == "branch":
        sid = args[1] if len(args) > 1 else ""
        at = ""
        if "--at" in args:
            i = args.index("--at")
            at = args[i + 1] if i + 1 < len(args) else ""
        try:
            s = store.Session.open(home, sid)
            s.branch(at)
            print(f"branched {sid} at {at}")
            return proto.EXIT_DONE
        except OSError as e:
            print(e, file=sys.stderr)
            return proto.EXIT_USAGE
    if sub == "tree":
        sid = args[1] if len(args) > 1 else ""
        try:
            evs = store.Session.open(home, sid).events()
        except OSError as e:
            print(e, file=sys.stderr)
            return proto.EXIT_USAGE
        by_parent: dict[str | None, list[dict]] = {}
        for e in evs:
            by_parent.setdefault(e.get("parent"), []).append(e)

        def walk(parent, depth):
            for e in by_parent.get(parent, []):
                print(f"{'  ' * depth}{e['id']} [{e['kind']}] {e['ts']}")
                walk(e["id"], depth + 1)

        walk(None, 0)
        return proto.EXIT_DONE
    print("usage: zdev session list|export|branch|tree", file=sys.stderr)
    return proto.EXIT_USAGE


def _memory_cmd(args: list[str]) -> int:
    ctx = t.Ctx(os.getcwd())
    sub = args[0] if args else ""
    if sub == "search":
        print(t.run(ctx, "memory_search", {"query": " ".join(args[1:])}))
        return proto.EXIT_DONE
    if sub == "add":
        ty = "map"
        words = []
        it = iter(args[1:])
        for a in it:
            if a == "--type":
                ty = next(it, "map")
            else:
                words.append(a)
        print(t.run(ctx, "memory_write",
                    {"type": ty, "text": " ".join(words), "scope": "zdev"}))
        return proto.EXIT_DONE
    print("usage: zdev memory search|add", file=sys.stderr)
    return proto.EXIT_USAGE


def _acp() -> int:
    """ACP stdio JSON-RPC (contract §11).

    Threading model: a daemon reader thread drains stdin and routes
      · replies (no `method`, `id` matching `pending`) → the waiting request()
      · `session/cancel` → per-session threading.Event (acts even mid-prompt)
      · everything else → `inbound` queue consumed by the main loop below
    so a server→client request (session/request_permission) can be answered
    while a prompt turn is still running on the main thread.
    """
    agents: dict[str, agent.Agent] = {}
    cancels: dict[str, threading.Event] = {}
    pending: dict[int, "queue.Queue"] = {}
    inbound: "queue.Queue" = queue.Queue()
    req_ids = itertools.count(10_000)
    send_lock = threading.Lock()

    def send(v):
        with send_lock:
            sys.stdout.write(json.dumps(v) + "\n")
            sys.stdout.flush()

    def _reader():
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            if "method" not in msg:                 # reply to our request
                q = pending.get(msg.get("id"))
                if q is not None:
                    q.put(msg)
                continue
            if msg.get("method") == "session/cancel":
                sid = (msg.get("params") or {}).get("sessionId", "")
                ev = cancels.get(sid)
                if ev is not None:
                    ev.set()
                if msg.get("id") is not None:       # request form → ack it
                    send({"jsonrpc": "2.0", "id": msg["id"],
                          "result": {"cancelled": True}})
                continue
            inbound.put(msg)
        inbound.put(None)                           # EOF

    threading.Thread(target=_reader, daemon=True).start()

    def request(method, params, timeout=600):
        """Server→client JSON-RPC request; blocks for the client's result."""
        rid = next(req_ids)
        q: "queue.Queue" = queue.Queue()
        pending[rid] = q
        send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        try:
            return q.get(timeout=timeout)
        except queue.Empty:
            return None
        finally:
            pending.pop(rid, None)

    def update_for(sid, ev):
        k, d = ev["kind"], ev["data"]
        if k == "assistant":
            u = {"sessionUpdate": "agent_message_chunk",
                 "content": {"type": "text", "text": d.get("text", "")}}
        elif k == "tool_call":
            u = {"sessionUpdate": "tool_call", "toolCallId": d.get("id", ""),
                 "title": d.get("name", ""), "input": d.get("input") or {}}
        elif k == "tool_result":
            u = {"sessionUpdate": "tool_call_update",
                 "toolCallId": d.get("tool_use_id", ""),
                 "title": d.get("name", ""),
                 "status": "failed" if d.get("is_error") else "completed",
                 "content": [{"type": "content", "content":
                              {"type": "text", "text": d.get("content", "")}}]}
        elif k == "usage":
            u = {"sessionUpdate": "usage",
                 "input": d.get("input", 0), "output": d.get("output", 0),
                 "cache_read": d.get("cache_read", 0),
                 "cache_write": d.get("cache_write", 0),
                 "usd": d.get("usd"), "cum_usd": d.get("cum_usd")}
        elif k == "plan":
            u = {"sessionUpdate": "plan", "plan": d.get("plan", "")}
        elif k == "mode":
            u = {"sessionUpdate": "mode",
                 "from": d.get("from", ""), "mode": d.get("to", "")}
        elif k == "permission_decision":
            u = {"sessionUpdate": "notice",
                 "text": ("permission granted: " if d.get("granted")
                          else "permission denied: ")
                         + d.get("tool", d.get("capability", ""))}
        elif k == "subagent_start":
            u = {"sessionUpdate": "notice",
                 "text": f"⎇ subagent: {d.get('prompt', '')[:120]}"}
        elif k == "subagent_end":
            u = {"sessionUpdate": "notice",
                 "text": f"⎇ subagent done: {d.get('child_session', '')[:8]}"}
        elif k == "compaction":
            u = {"sessionUpdate": "notice",
                 "text": (f"context compacted: {d.get('before', '?')} → "
                          f"{d.get('after', '?')} est. tokens")}
        else:
            return None
        return {"jsonrpc": "2.0", "method": "session/update",
                "params": {"sessionId": sid, "update": u}}

    def make_emit(sid):
        def emit(ev):
            n = update_for(sid, ev)
            if n:
                send(n)
        return emit

    # asker_ctx holds the tool_call id/input of the in-flight exec_tool call so
    # the asker (which only receives the tool name) can report them accurately.
    def make_asker(a, sid, asker_ctx):
        def asker(tool: str) -> bool:
            resp = request("session/request_permission", {
                "sessionId": sid,
                "toolCallId": asker_ctx.get("tid") or tool,
                "tool": tool,
                "input": asker_ctx.get("input") or {},
                "options": [
                    {"id": "allow", "name": "Allow once"},
                    {"id": "allow_always", "name": "Always allow"},
                    {"id": "deny", "name": "Deny"}]})
            outcome = ((resp or {}).get("result") or {}).get("outcome") or {}
            opt = outcome.get("optionId") or outcome.get("outcome") or "deny"
            if opt in ("allow_always", "always"):
                if tool not in a.opts.allow:
                    a.opts.allow.append(tool)   # session-scoped allowlist
                return True
            return opt in ("allow", "selected", "yes")
        return asker

    def run_turns(a, sid, emit, asker_ctx):
        """Provider turn + tool-exec loop. Returns an ACP stopReason."""
        cancel = cancels.setdefault(sid, threading.Event())
        for _ in range(a.opts.max_turns or 16):
            if cancel.is_set():
                return "cancelled"
            try:
                done = a.turn(emit)
            except Exception:
                return "refusal"
            if done:
                return "end_turn"
            calls = [b for b in (a.messages[-1].get("content") or [])
                     if b.get("type") == "tool_use"]
            if not calls:
                return "end_turn"
            for c in calls:
                if cancel.is_set():
                    return "cancelled"
                asker_ctx["tid"], asker_ctx["input"] = c["id"], c["input"]
                a.exec_tool(c["id"], c["name"], c["input"], emit)
                if c["name"] == "submit_plan":
                    return "end_turn"           # waits for session/approve
        return "max_turns"

    while True:
        req = inbound.get()
        if req is None:
            break
        method, rid = req.get("method", ""), req.get("id")
        params = req.get("params") or {}

        def result(r): send({"jsonrpc": "2.0", "id": rid, "result": r})
        def err(code, msg): send({"jsonrpc": "2.0", "id": rid,
                                  "error": {"code": code, "message": msg}})

        if method == "initialize":
            result({"protocolVersion": 1,
                    "agentInfo": {"name": "zdev", "version": "0.1.0"},
                    "agentCapabilities": {"loadSession": False,
                                          "promptCapabilities": {"embeddedContext": True}}})
        elif method == "session/new":
            cwd = params.get("cwd") or os.getcwd()
            o = proto.RunOpts()
            o.cwd = cwd
            if params.get("mode") in ("plan", "agent", "yolo"):
                o.mode = params["mode"]
                o.yolo = o.mode == "yolo"
            if params.get("model"):
                o.model = params["model"]
            if params.get("resume"):
                o.resume = params["resume"]
            try:
                a = agent.Agent(o, "zdev", cwd)
            except Exception as e:
                err(-32603, str(e))
                continue
            sid = a.session.id
            agents[sid] = a
            cancels[sid] = threading.Event()
            asker_ctx: dict = {}
            a.asker = make_asker(a, sid, asker_ctx)
            a._asker_ctx = asker_ctx
            result({"sessionId": sid, "model": a.model_alias, "mode": a.mode})
        elif method == "session/prompt":
            sid = params.get("sessionId", "")
            a = agents.get(sid)
            if a is None:
                err(-32602, "unknown sessionId")
                continue
            cancels.setdefault(sid, threading.Event()).clear()
            prompt = "\n".join(b.get("text", "")
                               for b in params.get("prompt", []))
            emit = make_emit(sid)
            a.send_user(prompt, emit)
            reason = run_turns(a, sid, emit, getattr(a, "_asker_ctx", {}))
            result({"stopReason": reason})
        elif method == "session/approve":
            # plan-mode approval: replays approve_plan + keeps running turns
            sid = params.get("sessionId", "")
            a = agents.get(sid)
            if a is None:
                err(-32602, "unknown sessionId")
                continue
            cancels.setdefault(sid, threading.Event()).clear()
            emit = make_emit(sid)
            a.approve_plan(emit)
            reason = run_turns(a, sid, emit, getattr(a, "_asker_ctx", {}))
            result({"stopReason": reason})
        elif method == "session/set_mode":
            sid = params.get("sessionId", "")
            a = agents.get(sid)
            mode = params.get("mode", "")
            if a is None:
                err(-32602, "unknown sessionId")
            elif mode not in ("plan", "agent", "yolo"):
                err(-32602, f"bad mode {mode!r} (plan|agent|yolo)")
            else:
                old, a.mode = a.mode, mode
                a.opts.yolo = mode == "yolo"
                a.session.append("mode", {"from": old, "to": mode})
                result({"mode": mode})
        elif method == "session/set_model":
            # in-place provider swap: session log + messages are untouched,
            # the next turn() simply speaks to the new provider.
            sid = params.get("sessionId", "")
            a = agents.get(sid)
            alias = params.get("model", "")
            if a is None:
                err(-32602, "unknown sessionId")
                continue
            found = store.provider_for(a.cfg, alias)
            if not found:
                err(-32602, f"unknown model {alias!r}")
                continue
            a.provider, model_id = found
            a.model_alias = f"{alias} ({model_id})"
            a.session.append("model", {"alias": alias, "id": model_id})
            result({"model": a.model_alias})
        elif method == "session/list_models":
            cfg = store.load_config(proto.cli_home())
            models = [{"alias": k,
                       "provider": m.get("provider", ""),
                       "id": m.get("id", ""),
                       "context": m.get("context", 0),
                       "free": not any((m.get("price") or {}).values())}
                      for k, m in sorted(cfg["models"].items())]
            result({"models": models, "default": cfg.get("default_model")})
        elif rid is not None:
            err(-32601, "method not found")
    return proto.EXIT_DONE


def _ui(args: list[str]) -> int:
    """Launch the Ink TUI in <repo>/ink via `bun run start` (inherits stdio)."""
    ink = os.environ.get("ZDEV_UI_DIR") or str(
        Path(__file__).resolve().parents[2] / "ink")
    if not Path(ink).is_dir():
        print(f"zdev ui: Ink UI not found at {ink}\n"
              "set ZDEV_UI_DIR to the ink/ directory", file=sys.stderr)
        return proto.EXIT_FAILED
    bun = shutil.which("bun")
    if not bun:
        print("zdev ui: bun not found on PATH — install from https://bun.sh "
              "or run `bun run start` inside the ink/ directory", file=sys.stderr)
        return proto.EXIT_FAILED
    try:
        return subprocess.call([bun, "run", "start", *args], cwd=ink)
    except KeyboardInterrupt:
        return proto.EXIT_DONE


def main() -> int:
    args = sys.argv[1:]
    if args and args[0] == "tools":
        if len(args) > 1 and args[1] == "dump":
            sys.stdout.buffer.write(t.tools_dump().encode("utf-8"))
            return proto.EXIT_DONE
        print("usage: zdev tools dump", file=sys.stderr)
        return proto.EXIT_USAGE
    if args and args[0] == "session":
        return _session_cmd(args[1:])
    if args and args[0] == "memory":
        return _memory_cmd(args[1:])
    if args and args[0] == "acp":
        return _acp()
    if args and args[0] == "ui":
        return _ui(args[1:])
    try:
        opts = _parse(args)
    except ValueError as e:
        print(f"{e}\n{HELP}", file=sys.stderr)
        return proto.EXIT_USAGE
    except SystemExit as e:
        return int(e.code or 0)
    try:
        if opts.prompt is not None:
            return agent.run_headless(opts, "zdev", os.getcwd())
        return repl(opts)
    except Exception as e:
        print(f"{e}", file=sys.stderr)
        return proto.EXIT_FAILED


if __name__ == "__main__":
    sys.exit(main())
