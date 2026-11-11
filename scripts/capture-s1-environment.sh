#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
DESTINATION="${1:-${SYLUX_EVIDENCE_DIR:-}}"
if [[ -z "$DESTINATION" ]]; then
  printf 'usage: %s EVIDENCE_DIRECTORY (or set SYLUX_EVIDENCE_DIR)\n' "$0" >&2
  exit 2
fi
mkdir -p -- "$DESTINATION"
DESTINATION="$(cd -- "$DESTINATION" && pwd)"

# Missing required build products are explicit blockers. Browser binaries belong
# to T004 and are recorded as pending here until that harness exists.
missing=0
{
  printf '# Sanitized S1 environment capture\n'
  printf '\n## Time (UTC)\n'; date --utc --iso-8601=seconds
  printf '\n## Kernel and architecture\n'; uname -a
  printf '\n## Filesystem and mount for worktree\n'
  findmnt --target "$ROOT" --output TARGET,SOURCE,FSTYPE,OPTIONS
  stat -f --printf='filesystem_type=%T block_size=%S blocks=%b free_blocks=%f\n' "$ROOT"
  printf '\n## Toolchain\n'
  for tool in rustc cargo node npm python3; do
    if command -v "$tool" >/dev/null 2>&1; then
      printf '\n### %s\n' "$tool"
      "$tool" --version || { printf 'BLOCKED: %s --version failed\n' "$tool"; missing=1; }
    else
      printf 'BLOCKED: required tool missing: %s\n' "$tool"; missing=1
    fi
  done
  printf '\n## Tool binary locations\n'
  for tool in cargo rustc node npm python3; do command -v "$tool" || true; done
  printf '\n## Rust native SQLite provenance\n'
  if [[ -x "$ROOT/target/debug/sqlite-provenance" ]]; then
    sqlite_tmp="$(mktemp -d)"
    "$ROOT/target/debug/sqlite-provenance" "$sqlite_tmp/provenance.sqlite3"
    rm -rf -- "$sqlite_tmp"
  else
    printf 'BLOCKED: sqlite-provenance binary not built; run bash scripts/check.sh first\n'; missing=1
  fi
  printf '\n## Browser builds (T004-owned qualification)\n'
  if [[ -x "$ROOT/web/node_modules/.bin/playwright" ]]; then
    "$ROOT/web/node_modules/.bin/playwright" --version
    "$ROOT/web/node_modules/.bin/playwright" install --list
  else
    printf 'PENDING T004: project-local Playwright package/browser harness unavailable\n'
  fi
  printf '\n## Static artifact inventory and digest\n'
  if [[ -d "$ROOT/web/dist" ]] && find "$ROOT/web/dist" -type f -print -quit | grep -q .; then
    (cd "$ROOT/web/dist" && find . -type f -print0 | sort -z | xargs -0 sha256sum)
  else
    printf 'BLOCKED: web/dist missing or empty; run bash scripts/check.sh first\n'; missing=1
  fi
  printf '\n## Process observation\n'
  ps -eo pid,ppid,comm,args | awk -v self="$$" '$2 == self && $3 == "sylux" { print $1, $2, $3, "[arguments redacted]" }'
  if (( missing )); then printf '\nBLOCKED: one or more required environment artifacts are missing.\n'; fi
} > "$DESTINATION/environment.txt"

printf 'Environment capture: %s/environment.txt\n' "$DESTINATION"
(( missing == 0 )) || exit 1
