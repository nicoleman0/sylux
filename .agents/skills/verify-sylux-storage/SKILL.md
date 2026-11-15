---
name: verify-sylux-storage
description: Verify Sylux uploads, private-file access, and Trash recovery through the existing Rust test harness.
---

# Verify Sylux storage

Use for changes to upload/download, account privacy, or Trash and restore. The test suite starts isolated loopback servers against disposable SQLite and payload roots; it does not use an installed or deployed Sylux instance.

## Launch and doctor

Use the versions pinned in [`rust-toolchain.toml`](../../../rust-toolchain.toml), [`.nvmrc`](../../../.nvmrc), and [`web/package.json`](../../../web/package.json). Cargo tests embed `web/dist`, so rebuild it after UI changes and whenever its freshness is uncertain:

```sh
npm --prefix web run build
```

Install web dependencies with `npm --prefix web ci` only on a fresh checkout or after dependency/toolchain changes. The doctor is a real owned-fixture product journey, not test discovery:

```sh
cargo test --locked --features test-harness --test contract_uploads \
  binary_upload_commits_fixed_metadata_and_downloads_exact_bytes -- --exact
```

It checks the fixture's authenticated `/me`, uploads synthetic bytes through the real HTTP routes, and verifies the downloaded bytes and digest. `-- --list` only confirms test names; it does not establish product readiness.

## Drive

Start with the [feature map](features/README.md), then open only the relevant feature entry. Run the exact command listed there from the repository root. The default storage run covers all three mapped areas:

```sh
cargo test --locked --features test-harness \
  --test contract_uploads \
  --test contract_private_storage \
  --test contract_isolation \
  --test integration_personal_storage \
  --test integration_retry_reconcile
```

The tests use direct HTTP contracts and integrations against real in-process routes, SQLite, and payload storage. `web/tests/desktop-upload.spec.ts` and `web/tests/trash.spec.ts` are separate mocked-API UI checks. `tests/browser/storage.spec.ts` is a real TLS browser suite for upload/download UI paths; it does not cover Trash or multi-account isolation. Do not report these layers as interchangeable.

## Evidence

Capture command output in a fresh directory under the system temporary directory. It survives the test fixtures' cleanup and stays outside the repository:

```sh
evidence_dir="$(mktemp -d "${TMPDIR:-/tmp}/sylux-storage.XXXXXXXX")"
set -o pipefail
cargo test --locked --features test-harness \
  --test contract_uploads \
  --test contract_private_storage \
  --test contract_isolation \
  --test integration_personal_storage \
  --test integration_retry_reconcile 2>&1 | tee "$evidence_dir/contracts.log"
printf 'Evidence: %s\n' "$evidence_dir/contracts.log"
```

Record the exact command, exit status, and evidence path in the task report. A passing functional run does not establish physical durability, upgrade safety, or deployment qualification.

## Cleanup and limits

Fixtures create unique roots below the OS temporary directory and call `FixtureRoot::cleanup_after_workers_exit` after joining owned workers. Fixture roots are explicitly cleaned on successful test paths; a failed or interrupted test may leave a root for inspection. Do not remove broad temporary-directory contents or evidence. Do not kill processes by name.

The focused Rust suites are Unix-oriented and intended to run on macOS and Linux. `scripts/check.sh` and `scripts/check-beta.sh` are not macOS substitutes: both require Linux x86_64 qualification. The real TLS browser fixture also requires OpenSSL, the pinned Playwright Chromium, NSS `certutil`, and an owned `S1_HARNESS_OUTPUT`; CI provisions browser/NSS tools on Linux. Follow the repo UI/browser instructions before driving a browser.

The five mapped suites passed on 15 November 2026 against `f0f644142dce434ea4a5343dbb604a9c1e033be3`. Rerun the affected journey for current changes; this record is not evidence for later source revisions.
