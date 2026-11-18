# Trash recovery

## User paths and preconditions

The UI moves an entry to Trash with `DELETE /entries/{id}`, lists deleted entries with `GET /trash`, and restores one with `POST /trash/{id}/restore`; the router is in [`src/http/mod.rs`](../../../../src/http/mod.rs). Mutations require the current `If-Match` value. See [`docs/trash.md`](../../../../docs/trash.md) for conflict and uncertain-outcome behavior.

## Drive

From the repository root, run:

```sh
cargo test --locked --features test-harness \
  --test contract_uploads explicit_replacement_preserves_prior_bytes_in_deleted_items_and_restores_them -- --exact
cargo test --locked --features test-harness \
  --test contract_uploads folder_total_tracks_real_upload_replacement_trash_and_restore -- --exact
cargo test --locked --features test-harness \
  --test integration_retry_reconcile folder_recovery_lost_trash_and_restore_replies_are_inspected_after_restart -- --exact
cargo test --locked --features test-harness \
  --test contract_uploads storage_usage_tracks_trash_restore_replacement_and_removal_exactly -- --exact
```

Or run the full [storage suite](README.md) to include the related upload, private-file, and isolation cases.

## Observable proof and limits

Contract checks verify replaced bytes remain in Trash, restore conflicts are visible, successful restore recovers the original bytes, and folder totals reflect trash/restore. The usage contract checks exact Files and Trash totals ([`docs/storage-usage.md`](../../../../docs/storage-usage.md)) and that only administrators read every account. The restart integration drops the mutation reply, restarts and reconciles the owned fixture, then verifies content is hidden after trash and exact again after restore.

The browser Trash tests in `web/tests/trash.spec.ts` use mocked API responses. They cover presentation, paging, and feedback behavior; they do not verify persisted Trash state. Do not automatically retry an uncertain trash or restore request; inspect fresh Files and Trash as documented in [`docs/trash.md`](../../../../docs/trash.md).

## Permanent removal

Run the real HTTP/storage removal matrix and binding migration checks:

```sh
cargo test --locked --features test-harness --test contract_uploads permanent_removal --no-fail-fast
cargo test --locked --features test-harness --test integration_schema --test integration_binding --no-fail-fast
```

The removal cases cover same-operation replay after a dropped real socket reply,
CSRF/Origin, foreign ownership, changed confirmation, expiry, stale previews,
effect-scoped fingerprints, hidden descendant content/search/archive denial,
shared references after a real fixture restart, and restored active bytes after
each cleanup fault. Runtime retry and shutdown tests join blocking cleanup before
closing SQLite. The commit-fault case also runs the real offline payload verifier
CLI after cleanup. Mocked browser recovery includes reload and deliberate exact
replay; it is separate from those persistence checks.

The binding suite starts the real `serve` process from v1, after the database
transition, and with a partial private manifest temporary file. It checks event
order and retained identity/data. Schema checks reject unbound migration and
unrecorded transitions. These are disposable-root functional checks; they do not
qualify Linux, container recovery, physical durability or production upgrades.
