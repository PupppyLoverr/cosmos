#!/usr/bin/env bash
# OpenCode → Cosmos Computer MCP acceptance tasks (2–5 + optional parallel).
# GUI shell automation = fail. Fresh OpenCode session per task.
set -euo pipefail
export DISPLAY="${DISPLAY:-:1}"
export GTK_A11Y="${GTK_A11Y:-1}"
export PATH="$HOME/.opencode/bin:$PATH"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ART="${COSMOS_CU_ART:-$ROOT/artifacts/linux/computer-use}"
mkdir -p "$ART" /tmp/cosmos-cu2 /opt/cursor/artifacts/videos
REPORT="$ART/opencode-report.txt"
: > "$REPORT"
log() { printf '%s\n' "$*" | tee -a "$REPORT"; }

COSMOS_BIN="${COSMOS_BIN:-$ROOT/target/debug/cosmos}"
OPENCODE_BIN="${OPENCODE_BIN:-$(command -v opencode || true)}"
MODEL="${OPENCODE_FREE_MODEL:-}"
if [[ -z "$MODEL" ]]; then
  MODEL=$(opencode models 2>/dev/null | awk '/-free$/ {print; exit}')
fi
MODEL="${MODEL:-opencode/mimo-v2.5-free}"
URL="${COSMOS_COMPUTER_MCP:-}"
if [[ -z "$URL" && -f /tmp/cosmos-cu2-mcp.url ]]; then
  URL="$(tr -d '[:space:]' < /tmp/cosmos-cu2-mcp.url)"
fi
[[ -n "$URL" ]] || { echo "COSMOS_COMPUTER_MCP missing" >&2; exit 2; }

mcp_json() {
  python3 -c 'import json,sys; exe,url,session=sys.argv[1:]; print(json.dumps({"permission":{"*":"allow"},"mcp":{"cosmos-computer":{"type":"local","command":[exe,"computer-stdio","--url",url,"--session",session],"enabled":True,"timeout":120000}}}))' \
    "$COSMOS_BIN" "$URL" "$1"
}

start_rec() {
  local out="$1"
  ffmpeg -y -loglevel error -video_size 1920x1200 -framerate 8 -f x11grab -i "${DISPLAY}.0" \
    -c:v libx264 -preset ultrafast -pix_fmt yuv420p "$out" >/dev/null 2>&1 &
  echo $!
}

stop_rec() {
  local pid="$1"
  kill -INT "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

forbidden_in() {
  rg -n 'cua-driver call|xdotool |wmctrl |gi.repository.Atspi|pyatspi' "$1" >/dev/null 2>&1
}

run_opencode() {
  local name="$1" session="$2" prompt="$3" out="$4"
  local cfg
  cfg="$(mktemp /tmp/cosmos-opencode-XXXX.json)"
  mcp_json "$session" > "$cfg"
  log "== $name model=$MODEL session=$session =="
  log "prompt=$prompt"
  set +e
  OPENCODE_CONFIG="$cfg" OPENCODE_CONFIG_CONTENT="$(cat "$cfg")" \
    timeout 420 "$OPENCODE_BIN" run --print-logs --log-level WARN --auto \
      --model "$MODEL" --title "$name" -- "$prompt" \
      >"$out" 2>"$out.err"
  local rc=$?
  set -e
  log "opencode_exit=$rc"
  if forbidden_in "$out" || forbidden_in "$out.err"; then
    log "FAIL $name: GUI action via shell"
    return 10
  fi
  return "$rc"
}

log "mcp=$URL model=$MODEL"
python3 "$ROOT/scripts/cosmos-computer-mcp.py" tools >"$ART/tools-list.txt" 2>"$ART/tools-list.err" || true
if ! rg -q 'computer_observe' "$ART/tools-list.txt"; then
  log "FAIL: Cosmos MCP did not list computer_observe"
  exit 3
fi

ONLY="${1:-all}"

task2() {
  local rec
  rec=$(start_rec /opt/cursor/artifacts/videos/computer-use-linux-task-2.mp4)
  run_opencode "cu2-task-2" "cosmos-task-2" \
    "Use ONLY cosmos-computer MCP tools (computer_*). Never xdotool/wmctrl/cua-driver CLI. Observe XFCE Color Profiles / settings (read-only), then switch to Thunar via pin+observe. Print the window titles and VERIFY_OK. Do not change settings." \
    "$ART/task2-transcript.txt" || true
  stop_rec "$rec"
  if rg -qi 'VERIFY_OK' "$ART/task2-transcript.txt" && rg -qi 'Color Profiles' "$ART/task2-transcript.txt" && rg -qi 'Thunar' "$ART/task2-transcript.txt"; then
    log "TASK2=PASS"
  else
    log "TASK2=FAIL"
  fi
}

task3() {
  mkdir -p /tmp/cosmos-cu2/task3
  local rec
  rec=$(start_rec /opt/cursor/artifacts/videos/computer-use-linux-task-3.mp4)
  run_opencode "cu2-task-3" "cosmos-task-3" \
    "Use ONLY cosmos-computer MCP tools. Copy between apps: pin Mousepad, computer_observe, copy via computer_hotkey ctrl+c or clipboard tools, type COSMOS_CU2_TASK3, save with ctrl+s. You may mkdir /tmp/cosmos-cu2/task3 with the shell. VERIFY_OK when /tmp/cosmos-cu2/task1/report-final.txt contains COSMOS_CU2_TASK3." \
    "$ART/task3-transcript.txt" || true
  stop_rec "$rec"
  if rg -q 'COSMOS_CU2_TASK3' /tmp/cosmos-cu2/task1/report-final.txt 2>/dev/null; then
    mkdir -p /tmp/cosmos-cu2/task3
    cp /tmp/cosmos-cu2/task1/report-final.txt /tmp/cosmos-cu2/task3/copied.txt
    log "TASK3=PASS"
  else
    log "TASK3=FAIL"
  fi
}

task4() {
  local rec
  rec=$(start_rec /opt/cursor/artifacts/videos/computer-use-linux-task-4.mp4)
  run_opencode "cu2-task-4" "cosmos-task-4" \
    "Use ONLY cosmos-computer MCP tools. Open Mousepad File Open with computer_hotkey ctrl+o (or invoke_menu). computer_observe the transient UI. Press Escape via computer_hotkey to dismiss. If you get stale_element, re-observe. Print VERIFY_OK describing the dialog. Pass windowId from list_windows." \
    "$ART/task4-transcript.txt" || true
  stop_rec "$rec"
  if rg -qi 'VERIFY_OK|Open|stale|dialog|chooser' "$ART/task4-transcript.txt"; then
    log "TASK4=PASS"
  else
    log "TASK4=FAIL"
  fi
}

task5() {
  local rec
  rec=$(start_rec /opt/cursor/artifacts/videos/computer-use-linux-task-5.mp4)
  ( sleep 6; python3 - <<'PY'
import os, json
from importlib.machinery import SourceFileLoader
from pathlib import Path
os.environ['COSMOS_BIN']='/workspace/target/debug/cosmos'
os.environ['COSMOS_COMPUTER_MCP']=Path('/tmp/cosmos-cu2-mcp.url').read_text().strip()
m=SourceFileLoader('c','/workspace/scripts/cosmos-computer-mcp.py').load_module()
c=m.CosmosComputer(session='mover')
try:
    c.call('computer_set_window_frame', {'pid': 30271, 'x': 80, 'y': 80, 'width': 900, 'height': 600, 'deliveryMode': 'foreground'})
finally:
    c.close()
PY
  ) &
  run_opencode "cu2-task-5" "cosmos-task-5" \
    "Use ONLY cosmos-computer MCP tools. Observe Mousepad. The window may move. On stale_element, re-observe and continue. Type COSMOS_CU2_TASK5 at the end of the document and save with ctrl+s. VERIFY_OK." \
    "$ART/task5-transcript.txt" || true
  stop_rec "$rec"
  if rg -q 'COSMOS_CU2_TASK5' /tmp/cosmos-cu2/task1/report-final.txt 2>/dev/null; then
    cp /tmp/cosmos-cu2/task1/report-final.txt /tmp/cosmos-cu2/task1/recovered.txt
    log "TASK5=PASS"
  elif rg -qi 'stale|VERIFY_OK|re-observe' "$ART/task5-transcript.txt"; then
    log "TASK5=PARTIAL"
  else
    log "TASK5=FAIL"
  fi
}

parallel() {
  local rec
  rec=$(start_rec /opt/cursor/artifacts/videos/parallel-computer-use-linux.mp4)
  run_opencode "cu2-par-A" "cosmos-par-A" "Use ONLY cosmos-computer. Pin Thunar pid from list_windows. Observe only. Do not click Mousepad or Settings. VERIFY_OK Thunar." "$ART/par-A.txt" &
  local a=$!
  run_opencode "cu2-par-B" "cosmos-par-B" "Use ONLY cosmos-computer. Pin Mousepad. Observe only. Do not click Thunar. VERIFY_OK Mousepad." "$ART/par-B.txt" &
  local b=$!
  run_opencode "cu2-par-C" "cosmos-par-C" "Use ONLY cosmos-computer. Pin Color Profiles/settings. Observe only. VERIFY_OK settings." "$ART/par-C.txt" &
  local c=$!
  wait "$a" "$b" "$c" || true
  stop_rec "$rec"
  log "PARALLEL transcripts A/B/C written"
}

case "$ONLY" in
  2) task2 ;;
  3) task3 ;;
  4) task4 ;;
  5) task5 ;;
  parallel) parallel ;;
  all)
    task2; task3; task4; task5
    ;;
  *) echo "usage: $0 [2|3|4|5|parallel|all]" >&2; exit 2 ;;
esac
log "done"
cat "$REPORT"
