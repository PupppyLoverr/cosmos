#!/usr/bin/env bash
# Start computer-probe (or a headed Cosmos) MCP endpoint and print the URL.
# Used by the CU2 OpenCode tests. Does not talk to raw cua-driver.
set -euo pipefail
export DISPLAY="${DISPLAY:-:1}"
export GTK_A11Y="${GTK_A11Y:-1}"
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
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROBE="${COSMOS_COMPUTER_PROBE:-$ROOT/target/debug/examples/computer-probe}"
if [[ ! -x "$PROBE" ]]; then
  echo "building computer-probe..." >&2
  cargo build -p zeron-ui --example computer-probe >&2
  PROBE="$ROOT/target/debug/examples/computer-probe"
fi
exec "$PROBE" serve
