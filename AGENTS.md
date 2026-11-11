<!-- bmad:context -->
<!-- Verified 2026-11-11 against 6408245d6348e5b0a1f6263c8bcd6f1d46808d77. -->

# Sylux

## Product and delivery

- Read `PRODUCT.md` for product direction. Favor a pleasant,
  fast, efficient and focused file server for self-hosters and small groups.
- Linear owns priorities and scope; use its Projects to group outcomes.
  See [direction and delivery](https://linear.app/corvus-dev/document/sylux-direction-and-delivery-5a323553eb7b).
- Build the smallest useful end-to-end slice. State the user outcome,
  exclusions and observable acceptance criteria in the issue; split work
  when a part can deliver value independently. Avoid speculative hooks.
- Prefer finishing work; keep only a few issues ready and normally 1–2
  in progress. These are defaults, not limits on useful parallel work.
- Treat priorities as guidance. Add blockers only for concrete technical
  prerequisites. Features need not wait for feedback, publication or rollout.
- Feature completion needs merged source and relevant verification.
  Publishing and deployment are separate work, with no fixed beta deadline.
- After self-review and relevant checks, agents may commit, push a feature
  branch and open a PR without further permission. Leave the PR open for
  the owner's review; merge or enable auto-merge only with explicit approval.
- Use the owner's configured Git identity for commits. The public contact
  address in the README is not a commit identity; do not override author or
  committer identity with Corvus Dev or its contact address.

## Where to start

- For installation or release changes, read `docs/docker-beta.md`.
  Read `docs/lxc.md` before native deployment work.
- For storage mutations, read `src/domain/coordinator.rs`,
  `src/persistence/mod.rs` and the affected contract tests.

## Build and verification

- Use the versions declared in `rust-toolchain.toml`, `.nvmrc`
  and `web/package.json`.
- During iteration, run the smallest checks that exercise the changed
  behavior first. Broaden checks for affected storage, permissions and
  recovery paths; keep required CI before merge. Docs-only changes need
  a diff and link review, not an application build.
- For UI changes, start with `npm --prefix web run check` and the affected
  browser journey. For Rust changes, start with a relevant filtered test;
  enable `test-harness` for contract and integration coverage.
- Before Cargo builds, tests or Clippy, ensure `web/dist` is current:
  `build.rs` embeds it. Run `npm --prefix web run build` after UI changes.
  Install dependencies with `npm --prefix web ci` on a fresh checkout or
  when dependencies or toolchains change; otherwise reuse the install.
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
- Reuse checks for unchanged inputs; rerun when changes or failures
  invalidate them. Report gaps and measure bottlenecks before optimizing.

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
