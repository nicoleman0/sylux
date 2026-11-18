#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

run() {
  printf '\n+ '
  printf '%q ' "$@"
  printf '\n'
  "$@"
}

for tool in cargo npm node python3; do
  command -v "$tool" >/dev/null 2>&1 || { printf 'required tool not found: %s\n' "$tool" >&2; exit 127; }
done

# Ignored files must not be tracked (force-added files slip past .gitignore).
tracked_ignored="$(git ls-files -i -c --exclude-standard)"
if [[ -n "$tracked_ignored" ]]; then
  printf 'Tracked files match .gitignore. Remove them from the index with `git rm --cached <path>`:\n%s\n' "$tracked_ignored" >&2
  exit 1
fi

# build.rs embeds web/dist, so build assets before Cargo checks.
run python3 scripts/test-ci-changes.py
run python3 scripts/test-ci-workflow.py
run python3 scripts/test-ci-comment-structured.py
run python3 scripts/test-account-authority-check.py
run npm --prefix web ci
run node --test scripts/test-ci-comment-syntax.mjs
run node --test scripts/test-measure-image-footprint.mjs
run node --test tools/test-browser-screenshot-paths.mjs
run npm --prefix web run check
run npm --prefix web run build

run cargo fmt --all -- --check
run cargo clippy --locked --all-targets -- -D warnings
run cargo test --locked --features test-harness --test contract_setup_auth --test integration_revocation
run cargo test --locked --features test-harness --test performance_fixture -- --list
run cargo test --locked
run cargo test --locked --features test-harness --bin sylux --test integration_admin_recovery --test integration_export --test integration_payload_verify --test integration_binding --test integration_local_lifecycle
run cargo build --locked

run npm --prefix web run audit:dependencies
run npm --prefix web run test:dependency-policy
run python3 tools/test-dependency-verifier.py
run python3 tools/test-rust-notices.py
run node --test tools/test-web-notices.mjs
run node --test scripts/workflow-measurement-helpers.test.mjs
artifact_check_pending=0
if [[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]]; then
  run python3 tools/verify-dependency-locks.py
else
  printf '\nLinux x86_64 dependency artifact qualification remains required.\n'
  artifact_check_pending=1
fi
run node tools/npm-native-smoke.mjs
CARGO_DENY_BIN="${CARGO_DENY_BIN:-cargo-deny}"
if [[ "$CARGO_DENY_BIN" == */* ]]; then
  [[ -x "$CARGO_DENY_BIN" ]] || { printf 'required cargo-deny executable is missing or not executable: %s\n' "$CARGO_DENY_BIN" >&2; exit 127; }
  CARGO_DENY_DIR="$(dirname -- "$CARGO_DENY_BIN")"
else
  command -v "$CARGO_DENY_BIN" >/dev/null 2>&1 || { printf 'required tool not found: %s\n' "$CARGO_DENY_BIN" >&2; exit 127; }
  CARGO_DENY_DIR="$(dirname -- "$(command -v "$CARGO_DENY_BIN")")"
fi
run "$CARGO_DENY_BIN" --version
run env PATH="$CARGO_DENY_DIR:$PATH" cargo --locked deny check advisories bans licenses sources
if (( artifact_check_pending )); then
  printf '\nChecks incomplete: run the full suite on Linux x86_64.\n' >&2
  exit 1
fi
