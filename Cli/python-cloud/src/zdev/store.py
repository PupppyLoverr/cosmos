"""Config (tomllib) + session store — same semantics as zcore (contract §1, §6)."""

from __future__ import annotations

import json
import os
import tomllib
from pathlib import Path

from . import proto


def load_config(home: str) -> dict:
    cfg: dict = {
        "default_model": None,
        "providers": {},
        "models": {},
        "permissions": {"allow": [], "ask": []},
        "subagents": {"max_parallel": 3},
        "bridge": {"large_read_lines": 200, "summarizer_model": ""},
        "mcp": {},
        "hooks": [],
    }
    path = Path(home) / "config.toml"
    if path.is_file():
        try:
            loaded = tomllib.loads(path.read_text(encoding="utf-8"))
            for k, v in loaded.items():
                if isinstance(cfg.get(k), dict) and isinstance(v, dict):
                    cfg[k].update(v)
                else:
                    cfg[k] = v
        except Exception as e:
            print(f"config.toml parse error: {e} — using defaults", file=__import__("sys").stderr)
    if not cfg["providers"]:
        cfg["providers"]["fixture"] = {"kind": "fixture", "base_url": "", "api_key_env": ""}
    if not cfg["models"]:
        cfg["models"]["fixture"] = {
            "provider": "fixture", "id": "fixture", "context": 128000,
            "price": {"input": 0, "cache_read": 0, "cache_write": 0, "output": 0},
        }
        cfg["default_model"] = "fixture"
    return cfg


def allowed_hosts(cfg: dict) -> list[str]:
    out = []
    for p in cfg["providers"].values():
        url = p.get("base_url", "")
        if "://" in url:
            host = url.split("://", 1)[1].split("/")[0].split(":")[0].split("?")[0]
            if host:
                out.append(host)
    return out


def provider_for(cfg: dict, alias: str) -> tuple[dict, str] | None:
    m = cfg["models"].get(alias)
    if not m:
        return None
    p = cfg["providers"].get(m["provider"])
    if not p:
        return None
    price = m.get("price", {})
    return {
        "kind": p.get("kind", "fixture"),
        "base_url": p.get("base_url", ""),
        "api_key_env": p.get("api_key_env", ""),
        "model": m["id"],
        "context": m.get("context", 128000),
        "prices": price,
    }, m["id"]


class Session:
    def __init__(self, home: str, sid: str, harness: str):
        self.home = home
        self.id = sid
        self.harness = harness
        self.file = Path(home) / "sessions" / f"{sid}.jsonl"
        self.leaf: str | None = None

    @classmethod
    def create(cls, home: str, harness: str) -> "Session":
        (Path(home) / "sessions").mkdir(parents=True, exist_ok=True)
        return cls(home, proto.new_id(), harness)

    @classmethod
    def open(cls, home: str, sid: str) -> "Session":
        s = cls(home, sid, "")
        if not s.file.is_file():
            raise FileNotFoundError(f"no session {sid}")
        evs = s.events()
        s.leaf = evs[-1]["id"] if evs else None
        s.harness = evs[0]["harness"] if evs else ""
        return s

    def events(self) -> list[dict]:
        try:
            return [
                json.loads(l)
                for l in self.file.read_text(encoding="utf-8").splitlines()
                if l.strip()
            ]
        except FileNotFoundError:
            return []

    def append(self, kind: str, data: dict) -> dict:
        ev = proto.event(self.id, self.harness, self.leaf, kind, data)
        with open(self.file, "a", encoding="utf-8", newline="\n") as f:
            f.write(proto.to_line(ev) + "\n")
        self.leaf = ev["id"]
        return ev

    def branch(self, at: str) -> dict:
        self.leaf = at
        return self.append("checkout", {"at": at})

    def chain(self) -> list[dict]:
        evs = self.events()
        by_id = {e["id"]: e for e in evs}
        out = []
        cur = evs[-1] if evs else None
        while cur:
            out.append(cur)
            cur = by_id.get(cur["parent"]) if cur.get("parent") else None
        out.reverse()
        return out

    def replay(self) -> list[dict]:
        """Parent-chain → provider messages [{role, content:[blocks]}]."""
        msgs = []
        for e in self.chain():
            k = e["kind"]
            d = e.get("data", {})
            if k == "user" and d.get("text"):
                msgs.append({"role": "user", "content": [{"type": "text", "text": d["text"]}]})
            elif k == "assistant":
                blocks = []
                if d.get("text"):
                    blocks.append({"type": "text", "text": d["text"]})
                for c in d.get("tool_calls") or []:
                    blocks.append({
                        "type": "tool_use", "id": c.get("id", ""),
                        "name": c.get("name", ""), "input": c.get("input", {}),
                    })
                if blocks:
                    msgs.append({"role": "assistant", "content": blocks})
            elif k == "tool_result":
                msgs.append({"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": d.get("tool_use_id", ""),
                    "content": d.get("content", ""),
                    "is_error": d.get("is_error", False),
                }]})
        return msgs

    @staticmethod
    def list(home: str) -> list[dict]:
        out = []
        d = Path(home) / "sessions"
        if not d.is_dir():
            return out
        for f in sorted(d.glob("*.jsonl")):
            evs = [
                json.loads(l)
                for l in f.read_text(encoding="utf-8").splitlines()
                if l.strip()
            ]
            if not evs:
                continue
            first_user = next(
                (e["data"].get("text", "") for e in evs if e["kind"] == "user"), ""
            )
            out.append({
                "id": f.stem, "harness": evs[0]["harness"],
                "started": evs[0]["ts"], "events": len(evs),
                "preview": first_user[:80],
            })
        out.sort(key=lambda s: s["started"])
        return out

    @staticmethod
    def export(home: str, sid: str, fmt: str = "md") -> str:
        f = Path(home) / "sessions" / f"{sid}.jsonl"
        text = f.read_text(encoding="utf-8")
        if fmt == "jsonl":
            return text
        evs = [json.loads(l) for l in text.splitlines() if l.strip()]
        md = f"# session {sid}\n\n"
        for e in evs:
            d = e.get("data", {})
            if e["kind"] == "user":
                md += f"## user · {e['ts']}\n\n{d.get('text', '')}\n\n"
            elif e["kind"] == "assistant":
                md += f"## assistant · {e['ts']}\n\n{d.get('text', '')}\n"
                for c in d.get("tool_calls") or []:
                    md += f"\n- `{c.get('name', '')}` {json.dumps(c.get('input', {}))}\n"
                md += "\n"
            elif e["kind"] == "tool_result":
                md += f"_{d.get('name', 'tool')}_ → `{d.get('content', '')[:200]}`\n\n"
        return md
