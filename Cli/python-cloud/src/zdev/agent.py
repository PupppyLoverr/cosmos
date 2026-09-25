"""Agent loop — same semantics as zcore::agent (contract §3-§9)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Callable

from . import proto, providers, store, tools as t

CONTRACT_MD = Path(__file__).parent.parent.parent.parent / "CONTRACT.md"


def system_snippet() -> str:
    try:
        text = CONTRACT_MD.read_text(encoding="utf-8")
        s = text.find("<!-- BEGIN SYSTEM -->") + len("<!-- BEGIN SYSTEM -->")
        e = text.find("<!-- END SYSTEM -->")
        if 0 < s < e:
            return text[s:e].strip()
    except OSError:
        pass
    return ("You are Cosmos, a coding agent running in the operator's terminal. "
            "You work through tools.")


def _find_meridian() -> str | None:
    if v := os.environ.get("MERIDIAN_BIN"):
        if os.path.isfile(v):
            return v
    for d in os.environ.get("PATH", "").split(os.pathsep):
        for n in ("meridian.exe", "meridian"):
            c = os.path.join(d, n)
            if os.path.isfile(c):
                return c
    return None


def boot_context(workspace: str) -> tuple[str, int, list[str]]:
    mer = _find_meridian()
    if mer:
        try:
            out = subprocess.run([mer, "boot", "--repo", workspace],
                                 capture_output=True, text=True, timeout=60)
            if out.returncode == 0:
                v = json.loads(out.stdout)
                return (v.get("context", ""), v.get("tokens", 0),
                        v.get("warnings") or [])
        except Exception:
            pass
    return (system_snippet(), 0,
            ["meridian bridge not found; boot context is the contract snippet only"])


def _run_hooks(cfg: dict, event: str, tool: str, payload: dict, session: str) -> str | None:
    ev = json.dumps({"event": event, "tool": tool, "data": payload, "session": session})
    for h in cfg.get("hooks") or []:
        if h.get("event") != event:
            continue
        m = h.get("matcher", "")
        if m not in ("", "*", tool):
            continue
        try:
            out = subprocess.run(h["command"], input=ev, shell=True,
                                 capture_output=True, text=True, timeout=60)
        except Exception:
            continue
        if out.returncode == 2:
            return out.stderr
        if out.returncode != 0:
            print(f"hook '{h['command']}' exited {out.returncode} — continuing",
                  file=sys.stderr)
    return None


def _cost(prices: dict, u: dict) -> float | None:
    if not any(prices.get(k) for k in ("input", "cache_read", "cache_write", "output")):
        return None
    return (u["input"] * prices.get("input", 0)
            + u["cache_read"] * prices.get("cache_read", 0)
            + u["cache_write"] * prices.get("cache_write", 0)
            + u["output"] * prices.get("output", 0)) / 1_000_000


class Agent:
    def __init__(self, opts: proto.RunOpts, harness: str, cwd: str):
        self.opts = opts
        self.home = proto.cli_home()
        self.cfg = store.load_config(self.home)
        self.workspace = opts.cwd or cwd
        alias = opts.model or self.cfg.get("default_model") or "fixture"
        found = store.provider_for(self.cfg, alias)
        if found:
            self.provider, model_id = found
        else:
            self.provider, model_id = (
                {"kind": "fixture", "base_url": "", "api_key_env": "",
                 "model": "fixture", "context": 128000, "prices": {}}, "fixture")
        self.model_alias = f"{alias} ({model_id})"
        self.mode = "yolo" if opts.yolo else (opts.mode or "agent")
        self.ctx = t.Ctx(self.workspace)
        self.ctx.mcp_servers = self.cfg.get("mcp") or {}
        self.ctx.large_read_lines = self.cfg["bridge"].get("large_read_lines", 200)
        self.ctx.no_sandbox = opts.no_sandbox
        self.ctx.escalations = list(opts.allow_escalation)
        self.ctx.net_allow = "network" in opts.allow_escalation
        if opts.resume:
            self.session = store.Session.open(self.home, opts.resume)
            self.session.harness = harness
            self.messages = self.session.replay()
        else:
            self.session = store.Session.create(self.home, harness)
            self.messages = []
        boot, self.boot_tokens, _ = boot_context(self.workspace)
        self.system = [boot] if "You are Cosmos" in boot else [system_snippet(), boot]
        self.tools = t.tool_specs()
        self.allowed = store.allowed_hosts(self.cfg)
        self.cum_usd = 0.0
        self.last_usage = {"input": 0, "cache_read": 0, "cache_write": 0, "output": 0}
        self.asker: Callable[[str], bool] | None = None
        self.pending_plan: str | None = None
        # task{worktree:true} checkouts — removed on cleanup only if clean (§10)
        self.worktrees: list[tuple[str, str]] = []

    def __del__(self):
        for wt, branch in getattr(self, "worktrees", []):
            try:
                clean = subprocess.run(
                    ["git", "-C", wt, "status", "--porcelain"],
                    capture_output=True, text=True)
                if clean.returncode == 0 and not clean.stdout.strip():
                    subprocess.run(["git", "-C", self.ctx.workspace,
                                    "worktree", "remove", wt],
                                   capture_output=True)
                    subprocess.run(["git", "-C", self.ctx.workspace,
                                    "branch", "-D", branch],
                                   capture_output=True)
            except Exception:
                pass

    def _permitted(self, name: str) -> str:
        if self.mode == "yolo":
            return "allow"
        if self.mode == "plan" and name not in t.PLAN_TOOLS:
            return "deny"
        if name in self.opts.allow or name in self.cfg["permissions"].get("allow", []):
            return "allow"
        if name in self.cfg["permissions"].get("ask", []):
            return "ask"
        return "deny"

    def send_user(self, text: str, emit) -> None:
        emit(self.session.append("user", {"text": text}))
        self.messages.append({"role": "user",
                              "content": [{"type": "text", "text": text}]})

    def approve_plan(self, emit) -> None:
        if self.pending_plan:
            plan, self.pending_plan = self.pending_plan, None
            self.mode = "agent"
            emit(self.session.append("mode", {"from": "plan", "to": "agent"}))
            self.messages.append({"role": "user", "content": [
                {"type": "text", "text": f"[approved plan]\n{plan}"}]})

    def turn(self, emit) -> bool:
        """One provider turn. True when no tool calls were made."""
        self._maybe_compact(emit)
        text, calls = "", []
        usage = {"input": 0, "cache_read": 0, "cache_write": 0, "output": 0}
        err = []

        def sink(c: dict):
            if c["type"] == "text":
                nonlocal text
                text += c["text"]
            elif c["type"] == "tool_use":
                calls.append(c)
            elif c["type"] == "done":
                for k in usage:
                    usage[k] += c["usage"].get(k, 0)
            elif c["type"] == "err":
                err.append(c["text"])

        providers.complete(
            self.provider,
            {"system": self.system, "tools": self.tools,
             "messages": self.messages, "session_id": self.session.id,
             "max_tokens": 4096},
            sink, self.allowed)
        if err:
            raise RuntimeError(f"provider stream error: {err[0]}")
        self.last_usage = usage
        cost = _cost(self.provider.get("prices") or {}, usage)
        self.cum_usd += cost or 0.0
        emit(self.session.append("assistant", {
            "text": text,
            "tool_calls": [{"id": c["id"], "name": c["name"], "input": c["input"]}
                           for c in calls]}))
        emit(self.session.append("usage", {
            "input": usage["input"], "cache_read": usage["cache_read"],
            "cache_write": usage["cache_write"], "output": usage["output"],
            "usd": cost, "cum_usd": self.cum_usd}))
        print(f"turn · in {usage['input']} · cache_r {usage['cache_read']} "
              f"· cache_w {usage['cache_write']} · out {usage['output']} · "
              f"{f'${cost:.4f}' if cost is not None else '$?'} · "
              f"Σ ${self.cum_usd:.4f} · ctx n/a", file=sys.stderr)
        blocks = ([{"type": "text", "text": text}] if text else [])
        blocks += [{"type": "tool_use", "id": c["id"], "name": c["name"],
                    "input": c["input"]} for c in calls]
        self.messages.append({"role": "assistant", "content": blocks})
        return not calls

    def exec_tool(self, tid: str, name: str, inp: dict, emit) -> tuple[str, bool]:
        emit(self.session.append("tool_call",
                                 {"id": tid, "name": name, "input": inp}))
        blocked = _run_hooks(self.cfg, "pre_tool", name, inp, self.session.id)
        if blocked is not None:
            text, is_error = blocked, True
        else:
            perm = self._permitted(name)
            if perm == "deny":
                text, is_error = (f"permission denied: {name} is not allowed in "
                                  f"{self.mode} mode"), True
            elif perm == "ask":
                self.session.append("permission_request", {"tool": name, "input": inp})
                granted = self.asker(name) if self.asker else False
                emit(self.session.append("permission_decision",
                                         {"tool": name, "input": inp, "granted": granted}))
                if granted:
                    text, is_error = self._dispatch(name, inp, emit)
                else:
                    text, is_error = (f"permission denied: {name} requires "
                                      "operator approval"), True
            else:
                text, is_error = self._dispatch(name, inp, emit)
        emit(self.session.append("tool_result", {
            "tool_use_id": tid, "name": name, "content": text, "is_error": is_error}))
        _run_hooks(self.cfg, "post_tool", name,
                   {"input": inp, "is_error": is_error}, self.session.id)
        self.messages.append({"role": "user", "content": [{
            "type": "tool_result", "tool_use_id": tid,
            "content": text, "is_error": is_error}]})
        return text, is_error

    def _dispatch(self, name: str, inp: dict, emit) -> tuple[str, bool]:
        if name == "submit_plan":
            plan = inp.get("plan", "")
            self.pending_plan = plan
            emit(self.session.append("plan", {"plan": plan}))
            return f"plan submitted ({len(plan)} chars)", False
        if name == "request_escalation":
            cap = inp.get("capability", "")
            granted = cap in self.ctx.escalations
            emit(self.session.append("permission_decision", {
                "tool": "request_escalation", "capability": cap, "granted": granted}))
            if granted:
                if cap == "network":
                    self.ctx.net_allow = True
                return f"escalation granted: {cap}", False
            return (f"escalation denied: {cap} — rerun with --allow-escalation {cap}",
                    True)
        if name == "task":
            return self._subagent(inp.get("prompt", ""),
                                  bool(inp.get("worktree")), emit), False
        out = t.run(self.ctx, name, inp)
        return out, out.startswith("ERROR")

    def _add_worktree(self, child_id: str) -> str:
        """git worktree add COSMOS_CLI_HOME/worktrees/<session>/<child>."""
        wt = os.path.join(self.home, "worktrees", self.session.id, child_id)
        branch = f"cosmos-task-{child_id}"
        p = subprocess.run(
            ["git", "-C", self.ctx.workspace, "worktree", "add", wt, "-b", branch],
            capture_output=True, text=True)
        if p.returncode != 0:
            raise RuntimeError(p.stderr.strip())
        self.worktrees.append((wt, branch))
        return wt

    def _child_ctx(self, workspace: str) -> t.Ctx:
        c = t.Ctx(workspace)
        c.escalations = list(self.ctx.escalations)
        c.net_allow = self.ctx.net_allow
        c.no_sandbox = self.ctx.no_sandbox
        c.large_read_lines = self.ctx.large_read_lines
        c.mcp_servers = dict(self.ctx.mcp_servers)
        return c

    def _subagent(self, prompt: str, worktree: bool, emit) -> str:
        child = store.Session.create(self.home, self.session.harness)
        wt_note = ""
        if worktree:
            try:
                self._add_worktree(child.id)
                wt_note = f"\n[worktree branch: cosmos-task-{child.id}]"
            except Exception as e:
                wt_note = f"\n[worktree unavailable: {e}]"
        child_ctx = (self._child_ctx(self.worktrees[-1][0])
                     if self.worktrees else None)
        emit(self.session.append("subagent_start", {
            "child_session": child.id,
            "worktree": self.worktrees[-1][0] if self.worktrees else None,
            "prompt": prompt[:200]}))
        child.append("session_start", {
            "model": self.model_alias, "mode": "agent", "sandbox": "appcontainer",
            "boot_tokens": self.boot_tokens, "parent_session": self.session.id})
        msgs = [{"role": "user", "content": [{"type": "text", "text": prompt}]}]
        result = "subagent turn cap reached"
        for _ in range(8):
            text, calls = "", []

            def sink(c: dict):
                nonlocal text
                if c["type"] == "text":
                    text += c["text"]
                elif c["type"] == "tool_use":
                    calls.append(c)

            try:
                providers.complete(
                    self.provider,
                    {"system": self.system, "tools": self.tools, "messages": msgs,
                     "session_id": child.id, "max_tokens": 2048},
                    sink, self.allowed)
            except Exception:
                result = "subagent provider error"
                break
            blocks = ([{"type": "text", "text": text}] if text else [])
            blocks += [{"type": "tool_use", "id": c["id"], "name": c["name"],
                        "input": c["input"]} for c in calls]
            msgs.append({"role": "assistant", "content": blocks})
            if not calls:
                result = text[:4000]
                break
            for c in calls:
                tctx = child_ctx or self.ctx
                out = (t.run(tctx, c["name"], c["input"])
                       if c["name"] in t.PLAN_TOOLS
                       else f"subagent tool denied: {c['name']}")
                msgs.append({"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": c["id"],
                    "content": out, "is_error": False}]})
        emit(self.session.append("subagent_end", {"child_session": child.id}))
        return result + wt_note

    def _maybe_compact(self, emit) -> None:
        def est(m):
            return sum(
                len(b.get("text", "") or b.get("content", "")) // 4
                for b in m["content"])
        total = sum(est(m) for m in self.messages) + self.boot_tokens
        if total <= int(self.provider.get("context", 128000) * 0.8):
            return
        before = total
        head, tail = 2, 6
        if len(self.messages) <= head + tail:
            return
        dropped = len(self.messages) - head - tail
        self.messages = (self.messages[:head]
                         + [{"role": "user", "content": [{"type": "text", "text":
                f"[compaction: {dropped} earlier messages elided — see session log]"}]}]
                         + self.messages[-tail:])
        _run_hooks(self.cfg, "pre_compact", "", {"before_tokens": before},
                   self.session.id)
        after = sum(est(m) for m in self.messages) + self.boot_tokens
        emit(self.session.append("compaction", {
            "before_tokens": before, "after_tokens": after, "reason": "context>80%"}))


def emitter(mode: str):
    def emit(ev: dict):
        if mode == "json-stream":
            print(proto.to_line(ev), flush=True)
        elif mode == "json":
            print(json.dumps(ev, indent=2), flush=True)
        else:
            if ev["kind"] == "assistant" and ev["data"].get("text"):
                print(ev["data"]["text"])
            elif ev["kind"] == "tool_result":
                print(f"  [{ev['data'].get('name')}] "
                      f"{str(ev['data'].get('content'))[:200]}", file=sys.stderr)
    return emit


def run_headless(opts: proto.RunOpts, harness: str, cwd: str) -> int:
    prompt = opts.prompt
    if prompt == "-":
        prompt = sys.stdin.read()
    if prompt is None:
        raise ValueError("run_headless needs a prompt")
    agent = Agent(opts, harness, cwd)
    emit = emitter(opts.output)
    emit(agent.session.append("session_start", {
        "model": agent.model_alias, "mode": agent.mode,
        "sandbox": "off" if agent.ctx.no_sandbox else "appcontainer",
        "boot_tokens": agent.boot_tokens, "resume": opts.resume}))
    _run_hooks(agent.cfg, "session_start", "", {}, agent.session.id)
    emit(agent.session.append("user", {"text": prompt}))
    _run_hooks(agent.cfg, "user_prompt", "", {"text": prompt}, agent.session.id)
    agent.messages.append({"role": "user",
                           "content": [{"type": "text", "text": prompt}]})

    for _ in range(opts.max_turns or 16):
        if opts.max_usd is not None and agent.cum_usd >= opts.max_usd:
            return proto.EXIT_BUDGET
        done = agent.turn(emit)
        if done:
            emit(agent.session.append("session_end", {"reason": "done"}))
            return proto.EXIT_DONE
        calls = [(b["id"], b["name"], b["input"])
                 for b in agent.messages[-1]["content"]
                 if b["type"] == "tool_use"]
        denied = False
        for tid, name, inp in calls:
            text, is_error = agent.exec_tool(tid, name, inp, emit)
            if name == "submit_plan" and not is_error:
                if opts.approve_plan:
                    agent.approve_plan(emit)
                else:
                    emit(agent.session.append("session_end",
                                              {"reason": "plan_submitted"}))
                    return proto.EXIT_DONE
            if is_error and text.startswith("permission denied"):
                denied = True
        if denied:
            emit(agent.session.append("session_end",
                                      {"reason": "permission_denied"}))
            return proto.EXIT_DENIED
    emit(agent.session.append("session_end", {"reason": "max_turns"}))
    return proto.EXIT_FAILED
