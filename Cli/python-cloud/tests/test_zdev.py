"""zdev conformance tests — tools, sessions, event schema."""

import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from zdev import proto, store, tools as t  # noqa: E402


def test_read_hashlines(tmp_path):
    (tmp_path / "a.txt").write_text("alpha\nbeta\ngamma\n")
    ctx = t.Ctx(str(tmp_path))
    out = t.run(ctx, "read", {"path": "a.txt"})
    assert "file_sha256=" in out and "lines=3" in out
    assert "|alpha" in out and "1:" in out


def test_write_read_edit(tmp_path):
    ctx = t.Ctx(str(tmp_path))
    t.run(ctx, "write", {"path": "b.txt", "content": "one\ntwo\nthree\n"})
    r = t.run(ctx, "read", {"path": "b.txt"})
    anchor = next(l.split("|")[0] for l in r.splitlines() if "|two" in l)
    e = t.run(ctx, "edit", {"path": "b.txt", "edits": [
        {"start": anchor, "end": anchor, "new_text": "TWO"}]})
    assert e.startswith("ok"), e
    assert (tmp_path / "b.txt").read_text() == "one\nTWO\nthree\n"


def test_stale_and_escape(tmp_path):
    (tmp_path / "c.txt").write_text("x\ny\n")
    ctx = t.Ctx(str(tmp_path))
    e = t.run(ctx, "edit", {"path": "c.txt", "file_sha256": "deadbeef",
                            "edits": [{"start": "1:aaaaaa", "end": "1:aaaaaa",
                                       "new_text": "z"}]})
    assert "STALE" in e
    e2 = t.run(ctx, "write", {"path": "c.txt", "content": "clobber"})
    assert "STALE" in e2
    esc = t.run(ctx, "read", {"path": "../outside.txt"})
    assert "escapes workspace" in esc


def test_session_roundtrip(tmp_path):
    home = str(tmp_path)
    s = store.Session.create(home, "zdev")
    s.append("session_start", {"model": "fixture"})
    s.append("user", {"text": "ping"})
    s.append("assistant", {"text": "pong", "tool_calls": []})
    reopened = store.Session.open(home, s.id)
    assert reopened.harness == "zdev"
    msgs = reopened.replay()
    assert len(msgs) == 2
    assert msgs[0]["role"] == "user"
    s2 = store.Session.open(home, s.id)
    evs = s2.events()
    s2.branch(evs[1]["id"])
    msgs2 = store.Session.open(home, s.id).replay()
    assert len(msgs2) == 1


def test_event_shape(tmp_path):
    s = store.Session.create(str(tmp_path), "zdev")
    ev = s.append("user", {"text": "hi"})
    assert set(ev) >= {"v", "id", "parent", "ts", "session", "harness", "kind", "data"}
    assert ev["v"] == 1 and ev["harness"] == "zdev" and ev["kind"] == "user"


def test_tools_dump_matches_contract():
    contract = (os.path.join(os.path.dirname(__file__), "..", "..",
                             "contracts", "tools.json"))
    with open(contract, "rb") as f:
        canonical = f.read().decode()
    assert t.tools_dump() == canonical


def test_allowlist():
    from zdev.providers import host_allowed
    host_allowed("https://api.openai.com/x", ["api.openai.com"])
    for bad in ("https://evil.com", "https://api.openai.com.evil.com"):
        try:
            host_allowed(bad, ["api.openai.com"])
            raise AssertionError(f"{bad} allowed")
        except ValueError:
            pass
