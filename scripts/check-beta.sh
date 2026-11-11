#!/usr/bin/env bash
set -Eeuo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || { echo 'Beta qualification requires Linux x86_64' >&2; exit 1; }
# Run after scripts/check.sh so the embedded UI matches the tested source.
command -v certutil >/dev/null || { echo 'certutil is required for isolated browser trust' >&2; exit 127; }
S1_CERTUTIL="$(command -v certutil)"
export S1_CERTUTIL
mkdir -p evidence/beta
S1_HARNESS_OUTPUT="$(mktemp -d "$ROOT/evidence/beta/candidate.XXXXXXXX")"
export S1_HARNESS_OUTPUT
printf 'Private beta evidence: %s\n' "$S1_HARNESS_OUTPUT"
cargo clippy --locked --features test-harness --all-targets -- -D warnings
node scripts/test-browser-reporter.mjs
cargo test --locked --features test-harness --no-fail-fast
npm --prefix web run smoke

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    printf '## Beta contract and browser checks passed\n\n'
    printf 'Candidate: %s\n\n' "${GITHUB_SHA:-local}"
    printf 'Feature-enabled lint, unit/contract/integration/fault tests, real TLS browser journeys and web smoke checks passed.\n'
    printf 'Installation and stopped-service recovery require the separate Linux container recovery job.\n'
  } >> "$GITHUB_STEP_SUMMARY"
fi
