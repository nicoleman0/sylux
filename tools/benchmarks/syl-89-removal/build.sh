#!/usr/bin/env bash
set -euo pipefail
repo=$1
revision=$2
scripts=$(cd "$(dirname "$0")" && pwd)
bench=${SYL89_EVIDENCE_DIR:?set SYL89_EVIDENCE_DIR to an isolated directory outside the checkout}
mkdir -p "$bench"
checkout=$(mktemp -d "$bench/source.XXXXXXXX")
git -C "$repo" archive "$revision" | tar -x -C "$checkout"
if [[ -n "${3:-}" ]]; then
    (cd "$checkout" && git apply "$3")
fi
mkdir -p "$checkout/web/dist"
cp -R "$repo/web/dist/." "$checkout/web/dist/"
python3 "$scripts/instrument.py" "$checkout"
(cd "$checkout" && cargo build --locked --release --no-default-features --bin sylux)
cp "$checkout/target/release/sylux" "$bench/sylux"
printf '%s %s\n' "$revision" "$checkout" > "$bench/source.txt"
