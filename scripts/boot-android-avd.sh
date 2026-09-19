#!/usr/bin/env bash
# Boot a stable Android emulator for Cosmos. Kill by PID only — never pkill
# a pattern that can match this command line.
set -euo pipefail
export ANDROID_HOME="${ANDROID_HOME:-/home/ubuntu/android-sdk}"
export ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-$ANDROID_HOME}"
export PATH="$ANDROID_HOME/emulator:$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH"
AVD="${1:-cosmos_api34}"
LOG="${EMULATOR_LOG:-/tmp/emulator-${AVD}-cu2.log}"

# Stop a previous hung qemu for this AVD by PID.
while read -r pid cmd; do
  [[ -z "${pid:-}" ]] && continue
  if [[ "$cmd" == *"-avd $AVD"* ]] || [[ "$cmd" == *"-avd ${AVD}"* ]]; then
    kill "$pid" 2>/dev/null || true
    sleep 1
    kill -9 "$pid" 2>/dev/null || true
  fi
done < <(ps -eo pid=,args= | awk '/qemu-system/ && !/awk/ {print $1, $0}')

# netsimd leftover from bluetooth packet streamer
while read -r pid; do
  [[ -z "${pid:-}" ]] && continue
  kill "$pid" 2>/dev/null || true
done < <(ps -eo pid=,args= | awk '/\/emulator\/netsimd / && !/awk/ {print $1}')

adb kill-server >/dev/null 2>&1 || true
adb start-server >/dev/null

# The previous hang was `-feature -Bluetooth` (bad name). Real flag:
# BluetoothEmulation. Also disable wifi virtio to skip netsim.
: > "$LOG"
nohup emulator -avd "$AVD" \
  -no-window \
  -no-snapshot-load \
  -no-snapshot-save \
  -gpu swiftshader_indirect \
  -accel on \
  -no-audio \
  -no-boot-anim \
  -memory 2048 \
  -cores 2 \
  -feature -BluetoothEmulation,-Wifi \
  -verbose \
  >"$LOG" 2>&1 &
EMU_PID=$!
echo "emulator pid=$EMU_PID log=$LOG"

# Wait for boot.
for i in $(seq 1 90); do
  state=$(adb get-state 2>/dev/null || true)
  boot=$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r' || true)
  echo "wait $i state=$state boot=$boot"
  if [[ "$state" == "device" && "$boot" == "1" ]]; then
    adb shell settings put global window_animation_scale 0 || true
    adb shell settings put global transition_animation_scale 0 || true
    adb shell settings put global animator_duration_scale 0 || true
    echo "BOOT_OK pid=$EMU_PID"
    exit 0
  fi
  if ! kill -0 "$EMU_PID" 2>/dev/null; then
    echo "emulator exited early" >&2
    tail -40 "$LOG" >&2 || true
    exit 1
  fi
  sleep 5
done
echo "BOOT_TIMEOUT" >&2
tail -40 "$LOG" >&2 || true
exit 2
