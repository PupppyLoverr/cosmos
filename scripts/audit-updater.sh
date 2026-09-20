#!/usr/bin/env bash
# Fail if any updater / release / packaging code path can reach the upstream
# Zeron distribution. Run in CI or by hand; exit 0 == clean.
#
#   scripts/audit-updater.sh
#
# Scope: the crates and scripts that download, stage, or apply builds. The
# landing site, README, and edge worker are deliberately out of scope — they
# describe the upstream product and never run inside a Cosmos install.

set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

paths=(
  crates/update
  crates/engine/src/lib.rs
  crates/ui/src/shell.rs
  apps/zeron/src/update_cli.rs
  apps/zeron/src/main.rs
  scripts/package-macos.sh
  scripts/package-linux.sh
  scripts/package-windows.ps1
  .github/workflows/release.yml
)

pattern='zeron\.sh/(releases|install)|zeronsh/zeron|comet-native-releases|wrangler.*r2 object put'

status=0
echo "== updater audit: upstream release surfaces =="
if hits="$(grep -rnE "$pattern" "${paths[@]}" 2>/dev/null)"; then
  # Doc comments explaining *why* the updater is disabled are allowed; code is not.
  code_hits="$(printf '%s\n' "$hits" | grep -vE ':[0-9]+:\s*(//|#|\*|//!)' || true)"
  if [[ -n "$code_hits" ]]; then
    echo "FAIL: upstream release references in executable code:"
    printf '%s\n' "$code_hits"
    status=1
  else
    echo "ok: only comments mention the upstream channel"
  fi
else
  echo "ok: no upstream release references"
fi

echo "== updater audit: fail-closed gate =="
if grep -qE 'pub fn updates_enabled\(\) -> bool \{\s*$' crates/update/src/lib.rs &&
  sed -n '/pub fn updates_enabled() -> bool {/,/}/p' crates/update/src/lib.rs | grep -q 'false'; then
  echo "ok: zeron_update::updates_enabled() is hard-wired to false"
else
  echo "FAIL: updates_enabled() no longer fails closed"
  status=1
fi

for site in \
  'crates/engine/src/lib.rs:zeron_update::updates_enabled()' \
  'crates/ui/src/shell.rs:zeron_update::updates_enabled()' \
  'apps/zeron/src/update_cli.rs:zeron_update::ensure_updates_enabled()'; do
  file="${site%%:*}"
  needle="${site#*:}"
  if grep -qF "$needle" "$file"; then
    echo "ok: $file gates on $needle"
  else
    echo "FAIL: $file no longer gates on $needle"
    status=1
  fi
done

[[ $status -eq 0 ]] && echo "PASS: no user-facing updater path can reach zeron.sh or zeronsh/zeron"
exit $status
