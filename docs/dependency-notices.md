# Dependencies and notices

Sylux is MIT licensed. Keep the [license](../LICENSE) with redistributed copies.
Exact dependency versions are recorded in `Cargo.lock` and `web/package-lock.json`.

## Distributed notices

The Docker image includes Rust dependency license and notice texts under
`/usr/share/doc/sylux/third-party/`, with an inventory of package versions.
Bundled JavaScript dependency notices are collected from the emitted modules and
served at `/licenses/third-party.txt`; the build fails if a bundled package lacks notices.
The runtime operating-system packages retain their notices under `/usr/share/doc`.
The unmodified Poppins and JetBrains Mono fonts include their OFL notices in
`web/public/licenses`; the application serves them at `/licenses/poppins.txt`
and `/licenses/jetbrains-mono.txt`.

Rust dependencies carry MIT, Apache-2.0, ISC, BSD and Unicode terms, including
multi-license obligations in ring and Unicode packages. Use the included texts,
not the product's MIT license, for those components. SQLite's bundled core is
public domain; its Rust wrappers and build support have their own licenses.

## Build-tool licensing

Rust, Node and npm versions are pinned. Node and npm run during build and testing;
they are not part of the server runtime. The Lightning CSS compiler is an
unmodified MPL-2.0 build dependency; its covered compiler files are not copied
into the runtime image. Distributing the build tools separately needs their
own notices and source-availability review.

PDF.js installs `@napi-rs/canvas` as an optional Node canvas backend. Its
Linux x86_64 glibc binding is included in the build-host archive and binary
hash checks, and the native smoke test draws a synthetic pixel. This binding
is not copied into the server image or browser assets. Redistributing these
build tools requires review of the canvas and embedded Skia components too.

## Accepted npm advisory

`npm audit` reports GHSA-fc5q-39xc-vjpf (picomatch ReDoS through single-branch
repeated extglobs, high, affected <=4.0.7). Locked 4.0.7 is the latest release;
no patch exists. picomatch is reached only through build tooling (fdir,
tinyglobby, vite, svelte-check); only static `web/dist` ships, and no
picomatch runs in the server. `tools/npm-audit.mjs` accepts exactly this
finding until 2026-12-01T00:00:00Z, then fails. When a patched picomatch is
published, update the lockfile and delete the exception. If none exists by
then, the owner decides whether to renew.

Sources: [SPDX license expressions](https://spdx.github.io/spdx-spec/v2.3/SPDX-license-expressions/),
[SQLite copyright](https://www.sqlite.org/copyright.html),
[Mozilla MPL FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/).
