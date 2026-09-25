"""zproto — wire types shared by every Cosmos CLI harness (CONTRACT.md §6)."""

from __future__ import annotations

import json
import os
import time
import secrets
from dataclasses import dataclass, field

CONTRACT_VERSION = 1

# exit codes (contract §7)
EXIT_DONE = 0
EXIT_FAILED = 1
EXIT_USAGE = 2
EXIT_DENIED = 3
EXIT_PROVIDER = 4
EXIT_BUDGET = 5

_CROCKFORD = "0123456789abcdefghjkmnpqrstvwxyz"


def new_id() -> str:
    """ULID: 48-bit ms timestamp + 80-bit randomness, Crockford base32."""
    t = int(time.time() * 1000) & ((1 << 48) - 1)
    r = secrets.token_bytes(10)
    bits = (t << 80) | int.from_bytes(r, "big")
    chars = []
    for _ in range(26):
        chars.append(_CROCKFORD[bits & 31])
        bits >>= 5
    return "".join(reversed(chars))


def now_ts() -> str:
    from datetime import datetime, timezone

    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.") + f"{int(time.time()*1000)%1000:03d}Z"


def event(session: str, harness: str, parent: str | None, kind: str, data: dict) -> dict:
    return {
        "v": CONTRACT_VERSION,
        "id": new_id(),
        "parent": parent,
        "ts": now_ts(),
        "session": session,
        "harness": harness,
        "kind": kind,
        "data": data,
    }


def to_line(ev: dict) -> str:
    return json.dumps(ev, separators=(",", ":"), sort_keys=False)


@dataclass
class RunOpts:
    prompt: str | None = None
    output: str = "text"  # text|json|json-stream
    mode: str | None = None  # plan|agent
    allow: list[str] = field(default_factory=list)
    yolo: bool = False
    no_sandbox: bool = False
    model: str | None = None
    max_turns: int | None = None
    max_usd: float | None = None
    cwd: str | None = None
    resume: str | None = None
    approve_plan: bool = False
    allow_escalation: list[str] = field(default_factory=list)


def cli_home() -> str:
    home = os.environ.get("COSMOS_CLI_HOME")
    if home:
        return home
    base = os.environ.get("USERPROFILE") or os.path.expanduser("~")
    return os.path.join(base, ".cosmos-cli")
