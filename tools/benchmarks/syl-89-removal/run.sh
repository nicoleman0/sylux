#!/usr/bin/env bash
set -euo pipefail
scripts=$(cd "$(dirname "$0")" && pwd)
bench=${SYL89_EVIDENCE_DIR:?set SYL89_EVIDENCE_DIR to the build evidence directory}
tag=$1
shift
{
    date -u '+%FT%TZ'
    uname -srm
    sysctl -n hw.model hw.ncpu
    uptime
    df -h "$bench"
    python3 --version
    python3 -c 'import sqlite3; print("fixture SQLite", sqlite3.sqlite_version)'
    rustc -Vv
} > "$bench/$tag-host.log"
(
    while :; do
        date -u '+%FT%TZ'
        uptime
        ps -A -o pid,pcpu,comm | sort -k2,2nr | sed -n '1,14p'
        sleep 10
    done
) > "$bench/$tag-load.log" &
monitor=$!
trap 'kill "$monitor" 2>/dev/null || true' EXIT
python3 "$scripts/measure.py" "$@" | tee "$bench/$tag.jsonl"
