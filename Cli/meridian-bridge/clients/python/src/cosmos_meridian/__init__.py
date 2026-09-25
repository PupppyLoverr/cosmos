"""Thin Python client for `meridian serve --stdio`.

Speaks newline-delimited JSON-RPC 2.0 to a `meridian` child process.
Resolution order for the binary: MERIDIAN_BIN env, the workspace-relative
build path (../target/{release,debug}/meridian.exe), then PATH.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import threading
from pathlib import Path
from typing import Any

_HERE = Path(__file__).resolve()
_CRATE = _HERE.parents[4]  # clients/python/src/cosmos_meridian -> crate root


def find_binary() -> str:
    env = os.environ.get("MERIDIAN_BIN")
    if env:
        return env
    for profile in ("release", "debug"):
        cand = _CRATE / "target" / profile / ("meridian.exe" if os.name == "nt" else "meridian")
        if cand.exists():
            return str(cand)
    found = shutil.which("meridian")
    if found:
        return found
    raise FileNotFoundError(
        "meridian binary not found; set MERIDIAN_BIN or build meridian-bridge"
    )


class MeridianError(Exception):
    pass


class Meridian:
    """One child process; calls are serialized with a lock."""

    def __init__(self, binary: str | None = None, vault: str | None = None):
        env = os.environ.copy()
        if vault:
            env["MERIDIAN_VAULT"] = vault
        self._proc = subprocess.Popen(
            [binary or find_binary(), "serve", "--stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            env=env,
            text=True,
            encoding="utf-8",
        )
        self._lock = threading.Lock()
        self._next_id = 0

    def call(self, method: str, **params: Any) -> Any:
        with self._lock:
            self._next_id += 1
            rid = self._next_id
            assert self._proc.stdin and self._proc.stdout
            self._proc.stdin.write(
                json.dumps(
                    {"jsonrpc": "2.0", "id": rid, "method": method, "params": params}
                )
                + "\n"
            )
            self._proc.stdin.flush()
            line = self._proc.stdout.readline()
        if not line:
            raise MeridianError("meridian closed stdout")
        resp = json.loads(line)
        if "error" in resp:
            raise MeridianError(resp["error"]["message"])
        return resp["result"]

    # One wrapper per RPC method (contract surface).
    def boot_context(self, repo: str, harness: str = "zdev") -> dict:
        return self.call("boot.context", repo=repo, harness=harness)

    def memory_search(
        self, query: str, k: int = 5, scope: str | None = None,
        seen_ids: list[str] | None = None,
    ) -> list[dict]:
        out = self.call(
            "memory.search", query=query, k=k, scope=scope, seen_ids=seen_ids or []
        )
        return out["results"]

    def memory_write(self, type: str, text: str, scope: str, src: str) -> str:
        return self.call(
            "memory.write", type=type, text=text, scope=scope, src=src
        )["id"]

    def cache_read(self, path: str, max_lines: int = 200) -> str:
        return self.call("cache.read", path=path, max_lines=max_lines)["summary"]

    def playbook_get(self, name: str) -> str:
        return self.call("playbook.get", name=name)["body"]

    def session_register(
        self, session_id: str, harness: str, log_path: str, title: str
    ) -> str:
        return self.call(
            "session.register",
            session_id=session_id,
            harness=harness,
            log_path=log_path,
            title=title,
        )["path"]

    def close(self) -> None:
        try:
            if self._proc.stdin:
                self._proc.stdin.close()
            self._proc.wait(timeout=5)
        except Exception:
            self._proc.kill()

    def __enter__(self) -> "Meridian":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()
