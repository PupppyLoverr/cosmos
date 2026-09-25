"""Contract §2 tools — identical semantics to ztools (Rust)."""

from __future__ import annotations

import fnmatch
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

TOOLS_JSON = (Path(__file__).parent / "tools.json").read_bytes().decode("utf-8")
RESULT_CAP = 30_000
LARGE_READ_LINES = 200
PLAN_TOOLS = {"read", "grep", "glob", "ls", "memory_search", "submit_plan", "tool_search"}


def tools_dump() -> str:
    return TOOLS_JSON


def tool_specs() -> list[dict]:
    return json.loads(TOOLS_JSON)


class Ctx:
    def __init__(self, workspace: str):
        self.workspace = os.path.realpath(workspace)
        self.read_state: dict[str, str] = {}
        self.escalations: list[str] = []
        self.net_allow = False
        self.no_sandbox = False
        self.sandbox_degraded: str | None = None
        self.large_read_lines = LARGE_READ_LINES
        self.mcp_servers: dict = {}


def cap_result(s: str) -> str:
    if len(s) <= RESULT_CAP:
        return s
    return f"{s[:RESULT_CAP]}\n[truncated: {len(s)} chars > {RESULT_CAP} cap]"


def resolve(ctx: Ctx, path: str) -> str:
    p = Path(path)
    joined = p if p.is_absolute() else Path(ctx.workspace) / p
    norm = os.path.normpath(str(joined))
    root = os.path.normpath(ctx.workspace)
    if not (norm == root or norm.startswith(root + os.sep)):
        raise ValueError(f"path escapes workspace: {path}")
    return norm


def _sha6(line: str) -> str:
    return hashlib.sha256(line.rstrip("\r\n").encode()).hexdigest()[:6]


def _read_file(path: str) -> tuple[str, str]:
    data = Path(path).read_bytes()
    return data.decode("utf-8", errors="replace"), hashlib.sha256(data).hexdigest()


def read(ctx: Ctx, inp: dict) -> str:
    path = resolve(ctx, inp.get("path", ""))
    text, sha = _read_file(path)
    lines = text.split("\n")
    n = len(lines) - 1 if lines and lines[-1] == "" and text.endswith("\n") else len(lines)
    start = max(1, int(inp.get("start") or 1))
    end = min(n, int(inp.get("end") or n))
    if "start" not in inp and "end" not in inp and n > ctx.large_read_lines:
        raise ValueError(
            f"large_read_guard: {n} lines > {ctx.large_read_lines} limit — read with "
            "start/end ranges (<=200 lines) or fetch the meridian cache.read summary"
        )
    out = f"file_sha256={sha} lines={n}\n"
    for i in range(start, end + 1):
        if i < 1 or i > n:
            break
        line = lines[i - 1]
        out += f"{i}:{_sha6(line)}|{line}\n"
    ctx.read_state[path] = sha
    return out.rstrip("\n")


def _parse_anchor(s: str) -> tuple[int, str]:
    n, h = s.split(":", 1)
    return int(n), h


def _fresh(lines: list[str], around: int, n: int) -> str:
    lo, hi = max(1, around - 5), min(n, around + 5)
    return "".join(f"{i}:{_sha6(lines[i-1])}|{lines[i-1]}\n" for i in range(lo, hi + 1))


def edit(ctx: Ctx, inp: dict) -> str:
    path = resolve(ctx, inp.get("path", ""))
    text, sha = _read_file(path)
    want = inp.get("file_sha256")
    if want and want != sha:
        raise ValueError(f"STALE: file_sha256 mismatch (want {want}, have {sha})")
    newline = "\r\n" if "\r\n" in text else "\n"
    lines = text.split("\n")
    n = len(lines)
    pending = []
    stale = ""
    for e in inp.get("edits", []):
        s_n, s_h = _parse_anchor(e.get("start", ""))
        e_n, e_h = _parse_anchor(e.get("end", ""))
        ok = (
            s_n >= 1 and e_n >= s_n and e_n <= n
            and _sha6(lines[s_n - 1]) == s_h and _sha6(lines[e_n - 1]) == e_h
        )
        if not ok:
            stale += f"anchor mismatch near line {s_n}..{e_n}:\n{_fresh(lines, s_n, n)}\n"
        else:
            pending.append((s_n, e_n, e.get("new_text", "")))
    if stale:
        raise ValueError(f"STALE\n{stale}")
    owned = list(lines)
    for s_n, e_n, new_text in sorted(pending, key=lambda p: -p[0]):
        repl = new_text.split("\n")
        if repl and repl[-1] == "" and new_text:
            repl.pop()
        owned[s_n - 1 : e_n] = repl
    body = newline.join(owned)
    if text.endswith(newline) and not body.endswith(newline):
        body += newline
    _atomic_write(path, body.encode())
    return f"ok {path}"


def write(ctx: Ctx, inp: dict) -> str:
    path = resolve(ctx, inp.get("path", ""))
    content = inp.get("content", "")
    if os.path.exists(path):
        _, sha = _read_file(path)
        if ctx.read_state.get(path) != sha:
            raise ValueError(f"STALE: {path} exists and was not read this session (or changed)")
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    _atomic_write(path, content.encode())
    _, sha = _read_file(path)
    ctx.read_state[path] = sha
    return f"ok {path}"


def _atomic_write(path: str, data: bytes) -> None:
    tmp = f"{path}.ztmp{os.getpid()}"
    with open(tmp, "wb") as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    try:
        os.replace(tmp, path)
    except OSError:
        Path(path).write_bytes(data)
        os.unlink(tmp)


def ls(ctx: Ctx, inp: dict) -> str:
    path = resolve(ctx, inp.get("path", "."))
    entries = sorted(
        (e.name + "/" if e.is_dir() else e.name) for e in Path(path).iterdir()
    )
    return "\n".join(entries)


def _gitignore_patterns(root: str) -> list[str]:
    pats = []
    gi = Path(root) / ".gitignore"
    if gi.is_file():
        for line in gi.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if line and not line.startswith("#"):
                pats.append(line.lstrip("/"))
    return pats


def _ignored(rel: str, pats: list[str]) -> bool:
    rel = rel.replace("\\", "/")
    for p in pats:
        if p.endswith("/"):
            if rel.startswith(p[:-1]) or f"/{p[:-1]}/" in f"/{rel}/":
                return True
        elif fnmatch.fnmatch(rel, p) or fnmatch.fnmatch(os.path.basename(rel), p):
            return True
    return False


def _walk_files(ctx: Ctx, root: str):
    pats = _gitignore_patterns(ctx.workspace)
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [
            d for d in dirnames
            if not _ignored(os.path.relpath(os.path.join(dirpath, d), ctx.workspace), pats)
        ]
        for fn in filenames:
            full = os.path.join(dirpath, fn)
            rel = os.path.relpath(full, ctx.workspace).replace("\\", "/")
            if not _ignored(rel, pats):
                yield full, rel


def glob(ctx: Ctx, inp: dict) -> str:
    pattern = inp.get("pattern", "*")
    out = []
    for _full, rel in _walk_files(ctx, ctx.workspace):
        if fnmatch.fnmatch(rel, pattern):
            out.append(rel)
        if len(out) >= 500:
            break
    return "\n".join(sorted(out))


def grep(ctx: Ctx, inp: dict) -> str:
    try:
        re_pat = re.compile(inp.get("pattern", ""))
    except re.error as e:
        raise ValueError(f"bad regex: {e}")
    root = resolve(ctx, inp["path"]) if inp.get("path") else ctx.workspace
    glob_pat = inp.get("glob")
    out = []
    for full, rel in _walk_files(ctx, root):
        if glob_pat and not fnmatch.fnmatch(rel, glob_pat):
            continue
        try:
            text = Path(full).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for i, line in enumerate(text.splitlines(), 1):
            if re_pat.search(line):
                out.append(f"{rel}:{i}:{line}")
                if len(out) >= 200:
                    return "\n".join(out) + "\n[cap: 200 matches]"
    return "\n".join(out)


def _find_bin(names: list[str], env: str) -> str | None:
    v = os.environ.get(env)
    if v and os.path.isfile(v):
        return v
    for d in os.environ.get("PATH", "").split(os.pathsep):
        for n in names:
            c = os.path.join(d, n)
            if os.path.isfile(c):
                return c
    exe_dir = os.path.dirname(os.environ.get("COSMOS_BIN_DIR", ""))
    for n in names:
        c = os.path.join(exe_dir, n)
        if os.path.isfile(c):
            return c
    return None


def _shell_argv(command: str) -> list[str]:
    for d in os.environ.get("PATH", "").split(os.pathsep):
        b = os.path.join(d, "bash.exe")
        if os.path.isfile(b):
            return [b, "-lc", command]
    return ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command]


def bash(ctx: Ctx, inp: dict) -> str:
    command = inp.get("command", "")
    timeout = min(int(inp.get("timeout_s") or 120), 600)
    argv = _shell_argv(command)
    if ctx.no_sandbox:
        out = subprocess.run(argv, cwd=ctx.workspace, capture_output=True,
                             timeout=timeout, text=True, errors="replace")
        return out.stdout + out.stderr
    zsb = _find_bin(["zsandbox.exe", "zsandbox"], "ZSANDBOX_BIN")
    if not zsb:
        return "ERROR: zsandbox.exe not found (set ZSANDBOX_BIN)"
    note = f"[sandbox:degraded: {ctx.sandbox_degraded}]\n" if ctx.sandbox_degraded else ""
    try:
        out = subprocess.run(
            [zsb, "exec", "--workspace", ctx.workspace,
             "--net", "allow" if ctx.net_allow else "deny",
             "--timeout", str(timeout), "--"] + argv,
            cwd=ctx.workspace, capture_output=True, timeout=timeout + 30,
            text=True, errors="replace",
        )
        s = note + out.stdout + out.stderr
        if out.returncode == 124:
            s += "\n[timeout]"
        return s
    except subprocess.TimeoutExpired:
        return note + "[timeout]"


def _meridian(ctx: Ctx, args: list[str]) -> str:
    mer = _find_bin(["meridian.exe", "meridian"], "MERIDIAN_BIN")
    if not mer:
        raise ValueError("meridian bridge binary not found (set MERIDIAN_BIN)")
    env = dict(os.environ)
    out = subprocess.run([mer] + args, capture_output=True, text=True,
                         errors="replace", env=env)
    if out.returncode != 0:
        raise ValueError(f"meridian {args[0]}: {out.stderr.strip()}")
    return out.stdout.strip()


def memory_search(ctx: Ctx, inp: dict) -> str:
    return _meridian(ctx, ["search", "--query", inp.get("query", ""),
                           "--k", str(inp.get("k", 5))])


def memory_write(ctx: Ctx, inp: dict) -> str:
    ty = inp.get("type", "")
    if not ty:
        return ("ERROR: missing required field: type "
                "(decision|preference|map|correction|playbook)")
    return _meridian(ctx, ["write", "--type", ty,
                           "--scope", inp.get("scope", "global"),
                           "--text", inp.get("text", "")])


def mcp_or_search(ctx: Ctx, name: str, inp: dict) -> str:
    if not ctx.mcp_servers:
        return "ERROR: no MCP servers configured"
    return "ERROR: mcp client not configured for this harness run"


def run(ctx: Ctx, name: str, inp: dict) -> str:
    try:
        fn = {
            "read": read, "edit": edit, "write": write, "ls": ls,
            "glob": glob, "grep": grep, "bash": bash,
            "memory_search": memory_search, "memory_write": memory_write,
        }.get(name)
        if fn:
            return cap_result(fn(ctx, inp))
        if name in ("tool_search", "mcp_call"):
            return cap_result(mcp_or_search(ctx, name, inp))
        if name in ("submit_plan", "task", "request_escalation"):
            raise ValueError(f"{name} is handled by the harness, not the tool layer")
        raise ValueError(f"unknown tool: {name}")
    except Exception as e:
        return f"ERROR: {e}"
