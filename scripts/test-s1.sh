#!/usr/bin/env bash
set -Eeuo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# No dependency installation, browser download, trust-store mutation or TLS bypass.
exec python3 scripts/test-s1-runner.py "$@"
