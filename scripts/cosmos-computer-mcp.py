#!/usr/bin/env python3
"""Drive Cosmos Computer MCP over `cosmos computer-stdio` (JSON-RPC NDJSON).

This is the acceptance path: OpenCode (or this verifier) → Cosmos Computer → Cua.
GUI actions via xdotool/cua-driver CLI are cheating and must fail the test.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from typing import Any

COSMOS = os.environ.get("COSMOS_BIN", os.path.join(os.path.dirname(__file__), "..", "target", "debug", "cosmos"))
URL = os.environ["COSMOS_COMPUTER_MCP"]
SESSION = os.environ.get("COSMOS_COMPUTER_SESSION", f"cosmos-run-{os.getpid()}")


def _rpc(proc: subprocess.Popen, method: str, params: dict | None, rpc_id: int) -> dict:
    msg = {"jsonrpc": "2.0", "id": rpc_id, "method": method, "params": params or {}}
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    if not line:
        raise RuntimeError("computer-stdio closed")
    return json.loads(line)


class CosmosComputer:
    def __init__(self, session: str | None = None, pin_pid: int | None = None):
        args = [os.path.abspath(COSMOS), "computer-stdio", "--url", URL, "--session", session or SESSION]
        if pin_pid is not None:
            args += ["--pin-pid", str(pin_pid)]
        self.proc = subprocess.Popen(
            args,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._id = 0
        hello = _rpc(self.proc, "initialize", {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "cosmos-cu2", "version": "0"}}, 1)
        self._id = 1
        self.server = hello.get("result", {}).get("serverInfo", {})

    def tools(self) -> list[str]:
        self._id += 1
        reply = _rpc(self.proc, "tools/list", {}, self._id)
        return [t["name"] for t in reply.get("result", {}).get("tools", [])]

    def call(self, name: str, arguments: dict | None = None) -> str:
        self._id += 1
        reply = _rpc(self.proc, "tools/call", {"name": name, "arguments": arguments or {}}, self._id)
        if "error" in reply:
            raise RuntimeError(reply["error"])
        result = reply.get("result", {})
        texts = []
        for block in result.get("content", []):
            if block.get("type") == "text":
                texts.append(block.get("text") or "")
        text = "\n".join(texts)
        if result.get("isError"):
            raise RuntimeError(text)
        return text

    def close(self) -> None:
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        self.proc.terminate()
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: cosmos-computer-mcp.py tools | call NAME [JSON]", file=sys.stderr)
        return 2
    cc = CosmosComputer()
    try:
        if argv[1] == "tools":
            names = cc.tools()
            print("server", cc.server)
            print("\n".join(names))
            print(f"[{len(names)} tools] session={SESSION}", file=sys.stderr)
            return 0
        if argv[1] == "call":
            name = argv[2]
            args = json.loads(argv[3]) if len(argv) > 3 else {}
            print(cc.call(name, args))
            return 0
        print("unknown command", file=sys.stderr)
        return 2
    finally:
        cc.close()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
