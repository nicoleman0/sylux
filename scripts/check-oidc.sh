#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

fixture=$(cargo test --locked --features test-harness --test oidc_fixture --no-run --message-format=json |
    node --input-type=module -e '
        import { createInterface } from "node:readline";
        let fixture;
        for await (const line of createInterface({ input: process.stdin })) {
            const event = JSON.parse(line);
            if (event.reason === "compiler-message") process.stderr.write(event.message.rendered ?? "");
            if (event.reason === "compiler-artifact" && event.target.name === "oidc_fixture") fixture = event.executable;
        }
        if (!fixture) throw new Error("Cargo did not build the OIDC fixture executable");
        console.log(fixture);
    ')

"$fixture" ordinary_filter
"$fixture" ordinary_filter --exact
"$fixture" --nocapture
"$fixture" --list

expect_missing_argument() {
    local diagnostic=$1 output
    shift
    if output=$("$fixture" --fixture "$@" 2>&1) || [[ "$output" != *"$diagnostic"* ]]; then
        printf 'OIDC fixture must reject incomplete arguments with: %s\n' "$diagnostic" >&2
        return 1
    fi
}
expect_missing_argument 'fixture directory required'
expect_missing_argument 'fixture mode required' unused
echo 'OIDC fixture invocation checks passed.'

bash scripts/qualify-keycloak.sh "$fixture"
