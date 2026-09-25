"""Round-trip test: Python client -> `meridian serve --stdio` -> vault."""

import os
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from cosmos_meridian import Meridian, find_binary  # noqa: E402


@pytest.fixture()
def vault(tmp_path):
    return str(tmp_path / "vault")


def test_roundtrip(vault, tmp_path):
    with Meridian(vault=vault) as m:
        mid = m.memory_write("decision", "python client roundtrip works", "repo:py", "pytest/1")
        assert mid
        hits = m.memory_search("roundtrip works", k=3)
        assert any(h["text"] == "python client roundtrip works" for h in hits)
        assert hits[0]["type"] == "decision"

        src = tmp_path / "f.py"
        src.write_text("import os\n\ndef f():\n    pass\n")
        summary = m.cache_read(str(src))
        assert "language: python" in summary
        assert "def f" in summary

        ctx = m.boot_context(str(tmp_path), harness="zdev")
        assert ctx["tokens"] <= 2000
        assert "Cosmos" in ctx["text"]


def test_secret_rejected(vault):
    with Meridian(vault=vault) as m:
        try:
            m.memory_write("decision", "key sk-abcdef123456", "repo:py", "pytest/1")
        except Exception as e:
            assert "rejected" in str(e)
        else:
            raise AssertionError("secret write should be rejected")


def test_binary_resolvable():
    assert Path(find_binary()).exists()
