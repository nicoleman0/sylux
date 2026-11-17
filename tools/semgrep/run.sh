#!/usr/bin/env bash
# Runs the pinned Semgrep with vendored rules. Offline, no metrics, no login.
set -Eeuo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
VENV="${SEMGREP_VENV:-$ROOT/target/semgrep-venv}"
if [[ ! -x "$VENV/bin/semgrep" ]]; then
  "${SEMGREP_PYTHON:-python3}" -m venv "$VENV"
  "$VENV/bin/python" -m pip install --quiet --disable-pip-version-check --require-hashes --no-deps -r tools/semgrep/requirements.txt
fi
export SEMGREP_SEND_METRICS=off SEMGREP_ENABLE_VERSION_CHECK=0
exec "$VENV/bin/semgrep" scan --config tools/semgrep/rules.yml --metrics=off \
  --disable-version-check --error --quiet --exclude tools/semgrep "$@" src tests web/src web/tests scripts tools
