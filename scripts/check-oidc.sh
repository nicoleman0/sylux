#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

server=$(cargo build --locked --bin sylux --message-format=json |
    node --input-type=module -e '
        import { createInterface } from "node:readline";
        let executable;
        for await (const line of createInterface({ input: process.stdin })) {
            const event = JSON.parse(line);
            if (event.reason === "compiler-message") process.stderr.write(event.message.rendered ?? "");
            if (event.reason === "compiler-artifact" && event.target.name === "sylux") executable = event.executable;
        }
        if (!executable) throw new Error("Cargo did not build the Sylux executable");
        console.log(executable);
    ')

bash scripts/qualify-keycloak.sh "$server"
