"""Providers per CONTRACT.md — openai-compatible / anthropic / google / fixture.
Every HTTP call goes through the §12 host allowlist. Streaming via SSE."""

from __future__ import annotations

import json
import os
import urllib.request
import urllib.error
from typing import Callable

Chunk = dict  # {"type": "text"|"tool_use"|"done"|"err", ...}
Sink = Callable[[Chunk], None]


def host_allowed(url: str, allowed: list[str]) -> None:
    host = url.split("://", 1)[-1].split("/")[0].split(":")[0].split("?")[0].lower()
    ok = any(host == a.lower() or host.endswith("." + a.lower()) for a in allowed)
    if not ok:
        raise ValueError(f"privacy: outbound host {host} not on the allowlist")


def _api_key(cfg: dict) -> str | None:
    env = cfg.get("api_key_env", "")
    if not env:
        return None
    return os.environ.get(env) or None


def _sse(resp, f):
    for raw in resp:
        line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
        if line.startswith("data:"):
            data = line[5:].strip()
            if data == "[DONE]":
                break
            if f(data):
                break


def _openai_messages(req: dict) -> list[dict]:
    out = [{"role": "system", "content": s} for s in req["system"]]
    for m in req["messages"]:
        if m["role"] == "user":
            for b in m["content"]:
                if b["type"] == "text":
                    out.append({"role": "user", "content": b["text"]})
                elif b["type"] == "tool_result":
                    out.append({"role": "tool", "tool_call_id": b["tool_use_id"],
                                "content": b["content"]})
        elif m["role"] == "assistant":
            text = "\n".join(b["text"] for b in m["content"] if b["type"] == "text")
            calls = [{"id": b["id"], "type": "function",
                      "function": {"name": b["name"], "arguments": json.dumps(b["input"])}}
                     for b in m["content"] if b["type"] == "tool_use"]
            msg: dict = {"role": "assistant"}
            if text:
                msg["content"] = text
            if calls:
                msg["tool_calls"] = calls
            out.append(msg)
    return out


def _openai(cfg: dict, req: dict, sink: Sink, allowed: list[str]) -> None:
    url = cfg["base_url"].rstrip("/") + "/chat/completions"
    host_allowed(url, allowed)
    tools = [{"type": "function", "function": {
        "name": t["name"], "description": t["description"],
        "parameters": t["input_schema"]}} for t in req["tools"]]
    body = {"model": cfg["model"], "messages": _openai_messages(req),
            "max_tokens": req["max_tokens"], "stream": True,
            "stream_options": {"include_usage": True},
            "prompt_cache_key": req["session_id"]}
    if tools:
        body["tools"] = tools
    headers = {"content-type": "application/json"}
    if k := _api_key(cfg):
        headers["authorization"] = f"Bearer {k}"
    r = urllib.request.Request(url, data=json.dumps(body).encode(), headers=headers)
    try:
        resp = urllib.request.urlopen(r, timeout=300)
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"provider {e.code}: {e.read()[:400].decode(errors='replace')}")
    acc: dict[int, dict] = {}
    usage_seen = False

    def on_data(data: str) -> bool:
        nonlocal usage_seen
        try:
            v = json.loads(data)
        except ValueError:
            return False
        if u := v.get("usage"):
            usage_seen = True
            sink({"type": "done", "usage": {
                "input": (u.get("prompt_tokens") or 0)
                - ((u.get("prompt_tokens_details") or {}).get("cached_tokens") or 0),
                "cache_read": (u.get("prompt_tokens_details") or {}).get("cached_tokens") or 0,
                "cache_write": 0,
                "output": u.get("completion_tokens") or 0,
            }})
        for ch in v.get("choices") or []:
            d = ch.get("delta") or {}
            if d.get("content"):
                sink({"type": "text", "text": d["content"]})
            for tc in d.get("tool_calls") or []:
                idx = tc.get("index", 0)
                slot = acc.setdefault(idx, {"id": "", "name": "", "args": ""})
                if tc.get("id"):
                    slot["id"] = tc["id"]
                fn = tc.get("function") or {}
                if fn.get("name"):
                    slot["name"] += fn["name"]
                if fn.get("arguments"):
                    slot["args"] += fn["arguments"]
        return False

    _sse(resp, on_data)
    for slot in acc.values():
        if slot["id"]:
            try:
                inp = json.loads(slot["args"] or "{}")
            except ValueError:
                inp = {}
            sink({"type": "tool_use", "id": slot["id"], "name": slot["name"], "input": inp})
    if not usage_seen:
        sink({"type": "done", "usage": {"input": 0, "cache_read": 0, "cache_write": 0, "output": 0}})


def _anthropic(cfg: dict, req: dict, sink: Sink, allowed: list[str]) -> None:
    url = cfg["base_url"].rstrip("/") + "/v1/messages"
    host_allowed(url, allowed)
    sys_blocks = [{"type": "text", "text": s} for s in req["system"]]
    if sys_blocks:
        sys_blocks[-1]["cache_control"] = {"type": "ephemeral"}
    tools = [{"name": t["name"], "description": t["description"],
              "input_schema": t["input_schema"]} for t in req["tools"]]
    if tools:
        tools[-1]["cache_control"] = {"type": "ephemeral"}
    msgs = []
    for m in req["messages"]:
        parts = []
        for b in m["content"]:
            if b["type"] == "text":
                parts.append({"type": "text", "text": b["text"]})
            elif b["type"] == "tool_use":
                parts.append({"type": "tool_use", "id": b["id"],
                              "name": b["name"], "input": b["input"]})
            elif b["type"] == "tool_result":
                parts.append({"type": "tool_result", "tool_use_id": b["tool_use_id"],
                              "content": [{"type": "text", "text": b["content"]}],
                              "is_error": b.get("is_error", False)})
        if parts:
            msgs.append({"role": "user" if m["role"] == "user" else "assistant",
                         "content": parts})
    body = {"model": cfg["model"], "system": sys_blocks, "messages": msgs,
            "tools": tools, "max_tokens": req["max_tokens"], "stream": True}
    headers = {"content-type": "application/json", "anthropic-version": "2023-06-01"}
    if k := _api_key(cfg):
        headers["x-api-key"] = k
    r = urllib.request.Request(url, data=json.dumps(body).encode(), headers=headers)
    try:
        resp = urllib.request.urlopen(r, timeout=300)
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"provider {e.code}: {e.read()[:400].decode(errors='replace')}")
    acc: dict[int, dict] = {}

    def on_data(data: str) -> bool:
        try:
            v = json.loads(data)
        except ValueError:
            return False
        ty = v.get("type", "")
        if ty == "content_block_start" and v.get("content_block", {}).get("type") == "tool_use":
            i = v.get("index", 0)
            acc[i] = {"id": v["content_block"].get("id", ""),
                      "name": v["content_block"].get("name", ""), "args": ""}
        elif ty == "content_block_delta":
            i = v.get("index", 0)
            d = v.get("delta") or {}
            if d.get("type") == "text_delta" and d.get("text"):
                sink({"type": "text", "text": d["text"]})
            elif d.get("type") == "input_json_delta" and i in acc:
                acc[i]["args"] += d.get("partial_json", "")
        elif ty in ("message_start", "message_delta"):
            u = (v.get("message") or {}).get("usage") or v.get("usage") or {}
            sink({"type": "done", "usage": {
                "input": u.get("input_tokens") or 0,
                "cache_read": u.get("cache_read_input_tokens") or 0,
                "cache_write": u.get("cache_creation_input_tokens") or 0,
                "output": u.get("output_tokens") or 0,
            }})
        return False

    _sse(resp, on_data)
    for slot in acc.values():
        if slot["name"]:
            try:
                inp = json.loads(slot["args"] or "{}")
            except ValueError:
                inp = {}
            sink({"type": "tool_use", "id": slot["id"], "name": slot["name"], "input": inp})


def _google(cfg: dict, req: dict, sink: Sink, allowed: list[str]) -> None:
    url = (cfg["base_url"].rstrip("/") + f"/v1beta/models/{cfg['model']}"
           ":streamGenerateContent?alt=sse")
    host_allowed(url, allowed)
    contents = []
    for m in req["messages"]:
        parts = []
        for b in m["content"]:
            if b["type"] == "text":
                parts.append({"text": b["text"]})
            elif b["type"] == "tool_use":
                parts.append({"functionCall": {"name": b["name"], "args": b["input"]}})
            elif b["type"] == "tool_result":
                parts.append({"functionResponse": {"name": "tool",
                              "response": {"output": b["content"]}}})
        if parts:
            contents.append({"role": "model" if m["role"] == "assistant" else "user",
                             "parts": parts})
    body = {"systemInstruction": {"parts": [{"text": s} for s in req["system"]]},
            "contents": contents,
            "tools": [{"functionDeclarations": [
                {"name": t["name"], "description": t["description"],
                 "parameters": t["input_schema"]} for t in req["tools"]]}]}
    headers = {"content-type": "application/json"}
    if k := _api_key(cfg):
        headers["x-goog-api-key"] = k
    r = urllib.request.Request(url, data=json.dumps(body).encode(), headers=headers)
    try:
        resp = urllib.request.urlopen(r, timeout=300)
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"provider {e.code}: {e.read()[:400].decode(errors='replace')}")

    def on_data(data: str) -> bool:
        try:
            v = json.loads(data)
        except ValueError:
            return False
        for cand in v.get("candidates") or []:
            for p in (cand.get("content") or {}).get("parts") or []:
                if p.get("text"):
                    sink({"type": "text", "text": p["text"]})
                if fc := p.get("functionCall"):
                    from .proto import new_id
                    sink({"type": "tool_use", "id": new_id(),
                          "name": fc.get("name", ""), "input": fc.get("args", {})})
        if u := v.get("usageMetadata"):
            sink({"type": "done", "usage": {
                "input": (u.get("promptTokenCount") or 0)
                - (u.get("cachedContentTokenCount") or 0),
                "cache_read": u.get("cachedContentTokenCount") or 0,
                "cache_write": 0,
                "output": u.get("candidatesTokenCount") or 0,
            }})
        return False

    _sse(resp, on_data)


def _fixture(cfg: dict, req: dict, sink: Sink) -> None:
    """Deterministic offline provider. `ping`→`pong`; `tool:NAME {json}`→call."""
    last = ""
    has_tool_result = False
    if req["messages"]:
        m = req["messages"][-1]
        texts = [b["text"] for b in m["content"] if b["type"] == "text"]
        last = " ".join(texts)
        has_tool_result = any(b["type"] == "tool_result" for b in m["content"])
    est = lambda s: len(s) // 4
    sys_t = sum(est(s) for s in req["system"])
    msg_t = sum(
        est(b["text"]) if b["type"] == "text" else 4
        for m in req["messages"] for b in m["content"]
    )
    sink({"type": "done", "usage": {"input": sys_t + msg_t, "cache_read": 0,
                                    "cache_write": 0, "output": 0}})
    if has_tool_result:
        sink({"type": "text", "text": "done."})
    elif last.startswith("tool:"):
        rest = last[5:]
        name, _, js = rest.partition(" ")
        try:
            inp = json.loads(js) if js else {}
        except ValueError:
            inp = {}
        from .proto import new_id
        sink({"type": "tool_use", "id": new_id(), "name": name, "input": inp})
    elif "ping" in last.lower():
        sink({"type": "text", "text": "pong"})
    else:
        sink({"type": "text", "text":
              "fixture: no live provider configured — received "
              f"{last[:80]!r}. Configure [providers] + [models] in config.toml for a real model."})
    sink({"type": "done", "usage": {"input": 0, "cache_read": 0, "cache_write": 0,
                                    "output": est(str(cfg)) // 8 + 8}})


def complete(cfg: dict, req: dict, sink: Sink, allowed: list[str]) -> None:
    kind = cfg.get("kind", "fixture")
    if kind == "fixture":
        _fixture(cfg, req, sink)
    elif kind == "openai":
        _openai(cfg, req, sink, allowed)
    elif kind == "anthropic":
        _anthropic(cfg, req, sink, allowed)
    elif kind == "google":
        _google(cfg, req, sink, allowed)
    else:
        raise ValueError(f"unknown provider kind: {kind}")
