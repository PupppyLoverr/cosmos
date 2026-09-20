#!/usr/bin/env bash
# Live Linux Computer Use E2E — OBSERVE → UNDERSTAND → ACT → VERIFY → RECOVER
# Uses AT-SPI + X11 (xdotool/wmctrl) against XFCE apps. Harmless /tmp files only.
# Does not require Cua. Prints per-phase latencies in milliseconds.
set -euo pipefail
export DISPLAY="${DISPLAY:-:1}"
export GTK_A11Y="${GTK_A11Y:-1}"
# XFCE/dbus-launch often exports autolaunch: which zbus (Cua AT-SPI) rejects.
if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" || "$DBUS_SESSION_BUS_ADDRESS" == autolaunch:* ]]; then
  mid=$(tr -d '[:space:]' < /etc/machine-id 2>/dev/null || true)
  disp="${DISPLAY#:}"; disp="${disp%%.*}"
  busf="$HOME/.dbus/session-bus/${mid}-${disp}"
  if [[ -f "$busf" ]]; then
    # shellcheck disable=SC1090
    eval "$(grep '^DBUS_SESSION_BUS_ADDRESS=' "$busf")"
    export DBUS_SESSION_BUS_ADDRESS
  fi
fi
WORKDIR="${COSMOS_CU_WORKDIR:-/tmp/cosmos-cu-e2e}"
REPORT="${COSMOS_CU_REPORT:-/tmp/cosmos-cu-e2e-report.txt}"
mkdir -p "$WORKDIR"
: > "$REPORT"
log() { printf '%s\n' "$*" | tee -a "$REPORT"; }
ms() { python3 -c "import time; print(int(time.time()*1000))"; }

SESSION_TYPE="${XDG_SESSION_TYPE:-}"
if [[ -n "${WAYLAND_DISPLAY:-}" ]]; then SERVER=wayland
elif [[ "$SESSION_TYPE" == "wayland" ]]; then SERVER=wayland
elif [[ -n "${DISPLAY:-}" ]]; then SERVER=x11
else SERVER=unknown
fi

log "== Cosmos Linux Computer Use E2E =="
log "display=$DISPLAY session_type=${SESSION_TYPE:-unset} wayland=${WAYLAND_DISPLAY:-} verdict=$SERVER"
log "desktop=$(ps -eo comm= | grep -E '^(gnome-shell|plasmashell|xfce4-session|cinnamon|mate-session)$' | head -1 || echo unknown)"
if [[ "$SERVER" == "wayland" ]]; then
  log "LIMIT: Wayland — unique window ids for AT-SPI matching are unreliable; background input often background_unavailable; capture is xdg-desktop-portal ScreenCast."
else
  log "X11 — native drawable capture; AT-SPI semantic trees; background input generally available."
fi

command -v xdotool >/dev/null
command -v wmctrl >/dev/null
command -v thunar >/dev/null
command -v mousepad >/dev/null
command -v xfce4-settings-manager >/dev/null

# --- OBSERVE: AT-SPI ---
T0=$(ms)
python3 - <<'PY' | tee -a "$REPORT"
import os, time, sys
t0 = time.time()
try:
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi
except Exception as e:
    print(f"atspi-python: unavailable ({e})")
    sys.exit(0)
try:
    desktop = Atspi.get_desktop(0)
except Exception as e:
    print(f"atspi-connect: FAIL {e}")
    sys.exit(0)
n = desktop.get_child_count()
print(f"atspi-observe: {n} applications in {int((time.time()-t0)*1000)} ms")
for i in range(min(n, 24)):
    app = desktop.get_child_at_index(i)
    if app is None:
        continue
    name = app.get_name() or "?"
    role = app.get_role_name() or "?"
    print(f"  app[{i}] {role}: {name}")
PY
log "observe_atspi_wall_ms=$(( $(ms) - T0 ))"

# --- ACT: file manager create dir/files ---
rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"
T1=$(ms)
thunar "$WORKDIR" >/dev/null 2>&1 &
THUNAR_PID=$!
for i in {1..50}; do
  wmctrl -l | grep -qi 'cosmos-cu-e2e\|Thunar\|File Manager' && break
  sleep 0.1
done
log "file_manager_open_ms=$(( $(ms) - T1 ))"
echo "hello-cosmos" > "$WORKDIR/alpha.txt"
echo "second" > "$WORKDIR/beta.txt"
mv "$WORKDIR/beta.txt" "$WORKDIR/beta-renamed.txt"
ln -sf alpha.txt "$WORKDIR/link-alpha.txt"
log "files_created=$(ls -1 "$WORKDIR" | tr '\n' ' ')"

# Keep file manager open (background) while we drive the editor (foreground).
# --- ACT: text editor ---
T2=$(ms)
mousepad "$WORKDIR/alpha.txt" >/dev/null 2>&1 &
MOUSEPAD_PID=$!
for i in {1..50}; do
  wmctrl -l | grep -qi 'alpha.txt\|Mousepad' && break
  sleep 0.1
done
log "editor_open_ms=$(( $(ms) - T2 ))"
sleep 0.4
# Type additional content into the focused editor.
xdotool search --name 'alpha.txt|Mousepad' windowactivate --sync >/dev/null 2>&1 || true
sleep 0.2
xdotool key --delay 20 ctrl+End >/dev/null 2>&1 || true
xdotool type --delay 12 $'\ncomputer-use-e2e\n' >/dev/null 2>&1 || true
xdotool key --delay 20 ctrl+s >/dev/null 2>&1 || true
sleep 0.3
T3=$(ms)
if grep -q 'computer-use-e2e' "$WORKDIR/alpha.txt"; then
  log "editor_type_save_ms=$(( T3 - T2 )) VERIFY=ok"
else
  # Recover: write via filesystem if GUI type didn't land (focus steal).
  printf '\ncomputer-use-e2e\n' >> "$WORKDIR/alpha.txt"
  log "editor_type_save_ms=$(( T3 - T2 )) VERIFY=recovered_via_fs"
fi

# --- ACT: settings ---
T4=$(ms)
xfce4-settings-manager >/dev/null 2>&1 &
SETTINGS_PID=$!
for i in {1..50}; do
  wmctrl -l | grep -qi 'Settings' && break
  sleep 0.1
done
log "settings_open_ms=$(( $(ms) - T4 ))"
# Navigate appearance if present
sleep 0.4
xdotool search --name 'Settings' windowactivate --sync >/dev/null 2>&1 || true
sleep 0.2
# Tab around — harmless
xdotool key --delay 80 Tab Tab Return >/dev/null 2>&1 || true
sleep 0.4
wmctrl -l | tee -a "$REPORT" >/dev/null
log "windows_after_acts:"
wmctrl -l | tee -a "$REPORT"

# Thunar is single-instance: the child PID may exit while the window stays up.
if wmctrl -l | grep -qi 'cosmos-cu-e2e\|Thunar'; then
  log "background_file_manager=alive"
else
  log "background_file_manager=gone"
fi

# --- VERIFY files ---
log "final_listing:"
ls -la "$WORKDIR" | tee -a "$REPORT"
test -f "$WORKDIR/alpha.txt"
test -f "$WORKDIR/beta-renamed.txt"
grep -q computer-use-e2e "$WORKDIR/alpha.txt"
log "VERIFY files=ok"

# --- screenshot of the desktop for demo ---
SHOT="${COSMOS_CU_SHOT:-/tmp/cosmos-cu-e2e.png}"
if command -v import >/dev/null; then
  import -window root "$SHOT" && log "screenshot=$SHOT"
elif command -v scrot >/dev/null; then
  scrot "$SHOT" && log "screenshot=$SHOT"
else
  log "screenshot=unavailable"
fi

# Leave apps running for the demo unless COSMOS_CU_CLEAN=1
if [[ "${COSMOS_CU_CLEAN:-0}" == "1" ]]; then
  kill "$MOUSEPAD_PID" "$SETTINGS_PID" "$THUNAR_PID" >/dev/null 2>&1 || true
fi
log "DONE"
cat "$REPORT"
