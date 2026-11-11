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

## Build tools and checks

Rust, Node and npm versions are pinned. Node and npm run during build and testing;
they are not part of the server runtime. The Lightning CSS compiler is an
unmodified MPL-2.0 build dependency; its covered compiler files are not copied
into the runtime image. Distributing the build tools separately needs their
own notices and source-availability review.

`deny.toml`, `tools/verify-dependency-locks.py`, and the npm audit policy enforce
reviewed dependency constraints. The npm policy includes a narrowly scoped,
time-limited build-tool advisory exception; a passing check does not mean an
unqualified clean audit. Run `scripts/check.sh` on Linux x86_64 to check current
results. Do not silently broaden policy exceptions when a dependency changes.

Sources: [SPDX license expressions](https://spdx.github.io/spdx-spec/v2.3/SPDX-license-expressions/),
[SQLite copyright](https://www.sqlite.org/copyright.html),
[Mozilla MPL FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/).
