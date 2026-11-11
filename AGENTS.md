<!-- bmad:context -->
<!-- Verified 2026-11-11 against 6408245d6348e5b0a1f6263c8bcd6f1d46808d77. -->

# Sylux

## Where to start

- For installation or release changes, read `docs/docker-beta.md`.
  Read `docs/lxc.md` before native deployment work.
- For storage mutations, read `src/domain/coordinator.rs`,
  `src/persistence/mod.rs` and the affected contract tests.

## Build and verification

- Use the versions declared in `rust-toolchain.toml`, `.nvmrc`
  and `web/package.json`.
- Build the UI before Cargo builds, tests or Clippy:
  `npm --prefix web ci`
  `npm --prefix web run build`
  `build.rs` embeds `web/dist`; rebuild it after UI changes.
- Use `scripts/check.sh` for baseline checks. It requires
  cargo-deny and intentionally exits unsuccessfully outside Linux
  x86_64 because dependency artifact qualification remains pending.
- Plain `cargo test --locked` skips feature-gated suites.
  For contract, integration and fault tests, use
  `cargo test --locked --features test-harness --no-fail-fast`.
- For full beta verification, follow `.github/workflows/ci.yml`:
  run `scripts/check.sh`, install its browser prerequisites, then
  run `scripts/check-beta.sh` on Linux x86_64.
- Treat web smoke tests, real TLS browser tests and container
  recovery as separate evidence. Report which actually ran.

## Storage and privacy boundaries

- Submit serving-time SQL through `SqliteWorkers`. Keep network
  I/O outside SQL closures and transactions.
- Preserve coordinator admission and reconciliation. After an
  uncertain commit, reconcile the same mutation rather than
  retrying its domain effect.
- Keep test-only controls behind the `test-harness` feature.
- Keep lifecycle contract events allowlisted and redacted.
  Never include credentials, identities, paths or raw errors.
- Use disposable data for verification. Follow the deployment
  guide before backup or restore work.
- Do not claim power-loss durability, upgrade safety or deployment
  qualification from functional tests alone.

<!-- /bmad:context -->
