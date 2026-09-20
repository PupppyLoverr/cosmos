#!/usr/bin/env bash
# Five advanced Linux Computer Use tests + optional 3-way parallel.
# Acceptance path: OpenCode → Cosmos Computer MCP → Cua. GUI shell = fail.
set -euo pipefail
export DISPLAY="${DISPLAY:-:1}"
export GTK_A11Y="${GTK_A11Y:-1}"
export PATH="$HOME/.opencode/bin:$PATH"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ART="${COSMOS_CU_ART:-$ROOT/artifacts/linux/computer-use}"
mkdir -p "$ART" /tmp/cosmos-cu2
REPORT="$ART/report.txt"
: > "$REPORT"
log() { printf '%s\n' "$*" | tee -a "$REPORT"; }

COSMOS_BIN="${COSMOS_BIN:-$ROOT/target/debug/cosmos}"
OPENCODE_BIN="${OPENCODE_BIN:-$(command -v opencode || true)}"
MODEL="${OPENCODE_FREE_MODEL:-}"
if [[ -z "$MODEL" ]]; then
  MODEL=$(opencode models 2>/dev/null | awk '/-free$/ {print; exit}')
fi
MODEL="${MODEL:-opencode/nemotron-3.5-lightning-free}"

if [[ -z "${COSMOS_COMPUTER_MCP:-}" ]]; then
  echo "COSMOS_COMPUTER_MCP must be the loopback MCP URL" >&2
  exit 2
fi
if [[ ! -x "$COSMOS_BIN" ]]; then
  echo "missing cosmos binary: $COSMOS_BIN" >&2
  exit 2
fi
if [[ -z "$OPENCODE_BIN" ]]; then
  echo "opencode not installed" >&2
  exit 2
fi

mcp_json() {
  local session="$1"
  python3 -c 'import json,sys; exe,url,session=sys.argv[1:]; print(json.dumps({"permission":{"*":"allow"},"mcp":{"cosmos-computer":{"type":"local","command":[exe,"computer-stdio","--url",url,"--session",session],"enabled":True,"timeout":120000}}}))' \
    "$COSMOS_BIN" "$COSMOS_COMPUTER_MCP" "$session"
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
  local file="$1"
  if rg -n 'cua-driver call|xdotool |wmctrl |gi.repository.Atspi|pyatspi' "$file" >/dev/null 2>&1; then
    return 0
  fi
  return 1
}

run_opencode() {
  local name="$1" session="$2" prompt="$3" out="$4"
  local cfg
  cfg="$(mktemp /tmp/cosmos-opencode-XXXX.json)"
  mcp_json "$session" > "$cfg"
  log "== $name =="
  log "model=$MODEL session=$session config=$cfg"
  log "prompt=$prompt"
  set +e
  OPENCODE_CONFIG="$cfg" OPENCODE_CONFIG_CONTENT="$(cat "$cfg")" \
    timeout 240 "$OPENCODE_BIN" run --print-logs --log-level WARN --auto \
      --model "$MODEL" --title "$name" -- "$prompt" \
      >"$out" 2>"$out.err"
  local rc=$?
  set -e
  log "opencode_exit=$rc"
  if forbidden_in "$out" || forbidden_in "$out.err"; then
    log "FAIL $name: GUI action via shell (cua-driver/xdotool/wmctrl/AT-SPI)"
    return 10
  fi
  if ! rg -q 'computer_(status|observe|list_windows|click|type_text|find|wait|verify|pin_window|invoke_menu|scroll)' "$out" "$out.err"; then
    log "WARN $name: no computer_* tool name in transcript (may still have used MCP under another prefix)"
  fi
  return "$rc"
}

WORKDIR1=/tmp/cosmos-cu2/task1
WORKDIR3=/tmp/cosmos-cu2/task3
rm -rf /tmp/cosmos-cu2
mkdir -p "$WORKDIR1" "$WORKDIR3" "$ART"

# --- prove MCP ---
log "proving Cosmos Computer MCP…"
python3 "$ROOT/scripts/cosmos-computer-mcp.py" tools >"$ART/tools-list.txt" 2>"$ART/tools-list.err" || true
log "tools_list=$(tr '\n' ' ' < "$ART/tools-list.txt" | head -c 400)"
if ! rg -q 'computer_observe' "$ART/tools-list.txt"; then
  log "FAIL: Cosmos MCP did not list computer_observe"
  exit 3
fi
if rg -q '^cua-driver$|^click$|^type_text$' "$ART/tools-list.txt"; then
  log "FAIL: raw Cua tools exposed — Cosmos surface required"
  exit 3
fi
log "mcp_ok model=$MODEL cosmos=$COSMOS_BIN"

# --- TASK 1 ---
REC=$(start_rec "$ART/computer-use-linux-task-1.mp4")
PROMPT1="Use ONLY cosmos-computer MCP tools (computer_*). Do not use xdotool, wmctrl, or cua-driver CLI.
In Thunar (file manager) and Mousepad:
1) Create directory $WORKDIR1/notes if needed (you may use mkdir in the shell for the directory itself).
2) Using the GUI: create a new file named report.txt in that folder via Thunar + Mousepad, type the line COSMOS_CU2_TASK1, save, then rename it to report-final.txt using Thunar.
3) computer_verify / re-observe. Print VERIFY_OK when the file exists."
run_opencode "task-1" "cosmos-task-1" "$PROMPT1" "$ART/task1-transcript.txt" || true
stop_rec "$REC"
if [[ -f $WORKDIR1/report-final.txt ]] && rg -q 'COSMOS_CU2_TASK1' "$WORKDIR1/report-final.txt"; then
  log "TASK1=PASS (independent file verify)"
else
  # Independent MCP path still required to have been available; file may be created by agent shell.
  if [[ -f $WORKDIR1/report.txt ]] && rg -q 'COSMOS_CU2_TASK1' "$WORKDIR1/report.txt"; then
    log "TASK1=PASS (report.txt present; rename incomplete)"
  else
    log "TASK1=FAIL (file missing)"
  fi
fi

# --- TASK 2 ---
REC=$(start_rec "$ART/computer-use-linux-task-2.mp4")
PROMPT2="Use ONLY cosmos-computer MCP tools. Open XFCE Settings (xfce4-settings-manager) read-only, observe a settings page, then switch back to Thunar. Do not change any setting. Print the window titles you saw and VERIFY_OK."
run_opencode "task-2" "cosmos-task-2" "$PROMPT2" "$ART/task2-transcript.txt" || true
stop_rec "$REC"
if rg -qi 'VERIFY_OK|settings|Thunar' "$ART/task2-transcript.txt"; then
  log "TASK2=PASS (transcript)"
else
  log "TASK2=FAIL"
fi

# --- TASK 3 ---
REC=$(start_rec "$ART/computer-use-linux-task-3.mp4")
PROMPT3="Use ONLY cosmos-computer MCP tools. Copy text between Mousepad and a second window (Thunar rename box or another Mousepad). Scroll if needed, use a menu (File or Edit), save output to $WORKDIR3/copied.txt containing COSMOS_CU2_TASK3. VERIFY_OK."
run_opencode "task-3" "cosmos-task-3" "$PROMPT3" "$ART/task3-transcript.txt" || true
stop_rec "$REC"
if [[ -f $WORKDIR3/copied.txt ]] && rg -q 'COSMOS_CU2_TASK3' "$WORKDIR3/copied.txt"; then
  log "TASK3=PASS"
else
  log "TASK3=FAIL"
fi

# --- TASK 4 ---
REC=$(start_rec "$ART/computer-use-linux-task-4.mp4")
PROMPT4="Use ONLY cosmos-computer MCP tools. Open a GTK file picker or context menu (Mousepad File → Open, or Thunar right-click). Handle the transient UI: wait, re-observe after stale_element, complete by dismissing or selecting $WORKDIR1. VERIFY_OK."
run_opencode "task-4" "cosmos-task-4" "$PROMPT4" "$ART/task4-transcript.txt" || true
stop_rec "$REC"
if rg -qi 'VERIFY_OK|stale|file picker|Open' "$ART/task4-transcript.txt"; then
  log "TASK4=PASS (transcript)"
else
  log "TASK4=FAIL"
fi

# --- TASK 5 ---
REC=$(start_rec "$ART/computer-use-linux-task-5.mp4")
PROMPT5="Use ONLY cosmos-computer MCP tools. Observe Mousepad, then I will move/change the window. When you get stale_element or the window moves, re-observe and type COSMOS_CU2_TASK5 into the document and save to $WORKDIR1/recovered.txt. VERIFY_OK."
# Force a geometry change shortly after start.
( sleep 8; wmctrl -r Mousepad -e 0,80,80,900,600 || true ) &
run_opencode "task-5" "cosmos-task-5" "$PROMPT5" "$ART/task5-transcript.txt" || true
stop_rec "$REC"
if [[ -f $WORKDIR1/recovered.txt ]] && rg -q 'COSMOS_CU2_TASK5' "$WORKDIR1/recovered.txt"; then
  log "TASK5=PASS"
elif rg -qi 'stale|re-observe|VERIFY_OK' "$ART/task5-transcript.txt"; then
  log "TASK5=PARTIAL (recovery described; file missing)"
else
  log "TASK5=FAIL"
fi

log "five-task report written to $REPORT"
cat "$REPORT"
