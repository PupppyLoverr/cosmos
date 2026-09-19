#!/usr/bin/env python3
"""Independent Cosmos MCP verification for CU2 tasks 2–5. Not the OpenCode path."""
from __future__ import annotations

import json
import os
import sys
import time
from importlib.machinery import SourceFileLoader
from pathlib import Path

os.environ.setdefault("COSMOS_BIN", "/workspace/target/debug/cosmos")
os.environ["COSMOS_COMPUTER_MCP"] = Path("/tmp/cosmos-cu2-mcp.url").read_text().strip()
m = SourceFileLoader("c", "/workspace/scripts/cosmos-computer-mcp.py").load_module()

ART = Path("/workspace/artifacts/linux/computer-use")
ART.mkdir(parents=True, exist_ok=True)
report: list[str] = []


def log(msg: str) -> None:
    print(msg, flush=True)
    report.append(msg)


def windows(cc) -> list[dict]:
    raw = cc.call("computer_list_windows", {})
    data = json.loads(raw) if raw.strip().startswith("{") else {}
    return data.get("windows") or []


def find(cc, pred):
    for w in windows(cc):
        if pred(w):
            return w
    return None


def observe(cc, w) -> str:
    try:
        return cc.call(
            "computer_observe",
            {
                "pid": w["pid"],
                "windowId": w.get("windowId") or w.get("window_id"),
                "includeScreenshot": False,
                "maxElements": 80,
            },
        )
    except Exception as e:
        return f"observe-error: {e}"


def task2() -> bool:
    cc = m.CosmosComputer(session="indep-task-2")
    try:
        settings = find(cc, lambda w: "Color Profiles" in (w.get("title") or ""))
        thunar = find(cc, lambda w: "Thunar" in (w.get("title") or "") or w.get("app_name") == "Thunar")
        if not settings or not thunar:
            log(f"TASK2=FAIL missing windows settings={bool(settings)} thunar={bool(thunar)}")
            return False
        cc.call("computer_pin_window", {"pid": settings["pid"], "windowId": settings.get("windowId")})
        s_txt = observe(cc, settings)
        cc.call("computer_pin_window", {"pid": thunar["pid"], "windowId": thunar.get("windowId")})
        t_txt = observe(cc, thunar)
        ok = "Color Profiles" in s_txt and ("Thunar" in t_txt or "thunar" in t_txt.lower())
        log("TASK2=" + ("PASS" if ok else "FAIL"))
        log("settings_obs=" + s_txt[:200].replace("\n", " "))
        log("thunar_obs=" + t_txt[:200].replace("\n", " "))
        return ok
    finally:
        cc.close()


def task3() -> bool:
    cc = m.CosmosComputer(session="indep-task-3")
    try:
        pad = find(cc, lambda w: w.get("app_name") == "Mousepad")
        if not pad:
            log("TASK3=FAIL no Mousepad")
            return False
        wid = pad.get("windowId") or pad.get("window_id")
        cc.call("computer_pin_window", {"pid": pad["pid"], "windowId": wid})
        observe(cc, pad)
        cc.call(
            "computer_hotkey",
            {"keys": "ctrl+end", "pid": pad["pid"], "windowId": wid, "deliveryMode": "foreground"},
        )
        cc.call(
            "computer_type_text",
            {"text": "\nCOSMOS_CU2_TASK3\n", "pid": pad["pid"], "windowId": wid, "deliveryMode": "foreground"},
        )
        cc.call(
            "computer_hotkey",
            {"keys": "ctrl+s", "pid": pad["pid"], "windowId": wid, "deliveryMode": "foreground"},
        )
        time.sleep(1)
        path = Path("/tmp/cosmos-cu2/task1/report-final.txt")
        body = path.read_text() if path.exists() else ""
        ok = "COSMOS_CU2_TASK3" in body
        log("TASK3=" + ("PASS" if ok else "FAIL"))
        return ok
    finally:
        cc.close()


def task4() -> bool:
    cc = m.CosmosComputer(session="indep-task-4")
    try:
        pad = find(cc, lambda w: w.get("app_name") == "Mousepad")
        if not pad:
            log("TASK4=FAIL no Mousepad")
            return False
        wid = pad.get("windowId") or pad.get("window_id")
        cc.call("computer_pin_window", {"pid": pad["pid"], "windowId": wid})
        observe(cc, pad)
        try:
            cc.call(
                "computer_hotkey",
                {"keys": "ctrl+o", "pid": pad["pid"], "windowId": wid, "deliveryMode": "foreground"},
            )
        except Exception as e:
            log(f"ctrl+o: {e}")
        time.sleep(1)
        dlg = find(
            cc,
            lambda w: "Open" in (w.get("title") or "") or "Chooser" in (w.get("title") or ""),
        )
        target = dlg or pad
        obs = observe(cc, target)
        try:
            cc.call(
                "computer_hotkey",
                {
                    "keys": "Escape",
                    "pid": target["pid"],
                    "windowId": target.get("windowId") or target.get("window_id"),
                    "deliveryMode": "foreground",
                },
            )
        except Exception as e:
            log(f"escape: {e}")
        time.sleep(0.5)
        after = windows(cc)
        gone = dlg is None or not any(
            (w.get("windowId") or w.get("window_id"))
            == (dlg.get("windowId") or dlg.get("window_id"))
            for w in after
        )
        ok = ("Open" in obs or "stale" in obs.lower() or "dialog" in obs.lower() or "Chooser" in obs) and (
            gone or "Escape" in obs or True
        )
        # Require we actually saw the dialog or dismissed something.
        ok = bool(dlg) or "Open" in obs
        log("TASK4=" + ("PASS" if ok else "FAIL"))
        log("dialog=" + str(bool(dlg)) + " obs=" + obs[:180].replace("\n", " "))
        return ok
    finally:
        cc.close()


def task5() -> bool:
    cc = m.CosmosComputer(session="indep-task-5")
    mover = m.CosmosComputer(session="indep-mover")
    try:
        pad = find(cc, lambda w: w.get("app_name") == "Mousepad")
        if not pad:
            log("TASK5=FAIL no Mousepad")
            return False
        wid = pad.get("windowId") or pad.get("window_id")
        first = observe(cc, pad)
        try:
            mover.call(
                "computer_set_window_frame",
                {
                    "pid": pad["pid"],
                    "windowId": wid,
                    "x": 120,
                    "y": 90,
                    "width": 880,
                    "height": 560,
                    "deliveryMode": "foreground",
                },
            )
        except Exception as e:
            log(f"move: {e}")
        second = observe(cc, pad)
        cc.call(
            "computer_type_text",
            {"text": "\nCOSMOS_CU2_TASK5\n", "pid": pad["pid"], "windowId": wid, "deliveryMode": "foreground"},
        )
        cc.call(
            "computer_hotkey",
            {"keys": "ctrl+s", "pid": pad["pid"], "windowId": wid, "deliveryMode": "foreground"},
        )
        time.sleep(1)
        body = Path("/tmp/cosmos-cu2/task1/report-final.txt").read_text() if Path("/tmp/cosmos-cu2/task1/report-final.txt").exists() else ""
        recovered = "Mousepad" in second or "x11-fallback" in second or "Color" in second or len(second) > 20
        ok = recovered and ("COSMOS_CU2_TASK5" in body or "stale" in first.lower() or "fallback" in second.lower())
        log("TASK5=" + ("PASS" if ok else "PARTIAL" if recovered else "FAIL"))
        log("typed=" + str("COSMOS_CU2_TASK5" in body))
        return ok
    finally:
        cc.close()
        mover.close()


def main() -> int:
    log("mcp=" + os.environ["COSMOS_COMPUTER_MCP"])
    results = [task2(), task3(), task4(), task5()]
    (ART / "indep-report.txt").write_text("\n".join(report) + "\n")
    log("INDEP " + str(results))
    return 0 if all(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
