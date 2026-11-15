# Uploads and downloads

## User paths and preconditions

The HTTP router exposes `POST /mutation-tickets`, `POST /uploads`, `PUT /uploads/{id}/content`, `POST /uploads/{id}/commit`, and `GET`/`HEAD /entries/{id}/content` in [`src/http/mod.rs`](../../../../src/http/mod.rs). Tests use synthetic accounts, sessions, and payload bytes; they do not need a running external server.

## Drive

From the repository root, build current web assets if `web/dist` may be stale, then run:

```sh
cargo test --locked --features test-harness --test contract_uploads
```

Key cases include `binary_upload_commits_fixed_metadata_and_downloads_exact_bytes`, `staged_progress_is_not_publication_and_admission_is_create_only`, `cancellation_joins_the_active_worker_and_hides_all_bytes`, `authorized_head_ranges_conditionals_and_cross_account_denial_are_exact`, and `empty_unicode_and_multi_megabyte_payloads_round_trip_exactly` in [`tests/contract/uploads.rs`](../../../../tests/contract/uploads.rs).

For account-scoped upload authorization in the seeded route matrix, also run:

```sh
cargo test --locked --features test-harness --test contract_isolation a03_unrelated_and_admin_route_matrix_denies_owner_private_state -- --exact
```

## Observable proof and limits

The contract assertions check successful commit metadata, exact downloaded bytes and SHA-256, expected access denial, and cancellation/restart outcomes. [`tests/integration/personal_storage.rs`](../../../../tests/integration/personal_storage.rs) adds setup, login, Unicode/empty/binary uploads, and exact-byte checks after reconciliation restart.

`npm --prefix web run smoke -- desktop-upload.spec.ts replacement.spec.ts` checks UI flows against mocked APIs. It does not prove the Rust route or persistence behavior. The real TLS browser fixture covers real UI upload/download paths but requires its browser and NSS prerequisites; see [the top-level skill](../SKILL.md#cleanup-and-limits).
