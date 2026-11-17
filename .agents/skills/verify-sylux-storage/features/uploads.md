# Uploads and downloads

## User paths and preconditions

The HTTP router exposes `POST /mutation-tickets`, `POST /uploads`, `PUT`/`PATCH /uploads/{id}/content`, `POST /uploads/{id}/commit`, and `GET`/`HEAD /entries/{id}/content` in [`src/http/mod.rs`](../../../../src/http/mod.rs). Tests use synthetic accounts, sessions, and payload bytes; they do not need a running external server.

## Drive

From the repository root, build current web assets if `web/dist` may be stale, then run:

```sh
cargo test --locked --features test-harness --test contract_uploads
```

Resume and cleanup cases include `sequential_append_resumes_acknowledged_prefix_and_rejects_stale_offsets`, `interrupted_append_preserves_offset_isolation_checksum_and_expiry`, `upload_limit_accepts_64_gib_and_rejects_one_byte_over`, `restart_recovers_prepared_partial_before_and_after_publication`, `empty_append_commits_and_wrong_declared_digest_rejects_without_visibility`, `expired_partial_cleanup_continues_past_an_invalid_candidate`, `stalled_chunk_releases_its_writer_for_resume_and_expiry_cleanup`, `cancel_during_chunk_receive_reports_upload_canceled`, `malformed_append_headers_are_rejected_before_any_claim`, `final_chunk_publication_fault_recovers_through_the_handler`, `repeated_and_racing_commits_of_one_upload_apply_once`, `partial_cleanup_runs_at_start_marks_cleaned_rows_and_joins_on_shutdown`, `storage_shutdown_releases_and_joins_an_in_flight_append`, `status_reports_a_writer_until_its_registration_is_released`, `staged_then_canceled_payloads_are_reclaimed_after_rejection_commits`, `payload_reclaim_keeps_committed_uncertain_recovery_and_referenced_payloads` and `payload_reclaim_resumes_after_restart_at_each_cleanup_boundary`.

Key cases include `binary_upload_commits_fixed_metadata_and_downloads_exact_bytes`, `staged_progress_is_not_publication_and_admission_is_create_only`, `cancellation_joins_the_active_worker_and_hides_all_bytes`, `authorized_head_ranges_conditionals_and_cross_account_denial_are_exact`, and `empty_unicode_and_multi_megabyte_payloads_round_trip_exactly` in [`tests/contract/uploads.rs`](../../../../tests/contract/uploads.rs).

For account-scoped upload authorization in the seeded route matrix, also run:

```sh
cargo test --locked --features test-harness --test contract_isolation a03_unrelated_and_admin_route_matrix_denies_owner_private_state -- --exact
```

## Observable proof and limits

The contract assertions check successful commit metadata, exact downloaded bytes and SHA-256, expected access denial, and cancellation/restart outcomes. [`tests/integration/personal_storage.rs`](../../../../tests/integration/personal_storage.rs) adds setup, login, Unicode/empty/binary uploads, and exact-byte checks after reconciliation restart.

`npm --prefix web run smoke -- desktop-upload.spec.ts replacement.spec.ts resumable-upload.spec.ts` checks UI flows against mocked APIs. It does not prove the Rust route or persistence behavior. The real TLS browser fixture (`tests/browser/storage.spec.ts`) covers real UI upload/download and resume paths but requires its browser and NSS prerequisites; see [the top-level skill](../SKILL.md#cleanup-and-limits).

For large-file resume, throughput and server RSS (single and three concurrent uploads), run `python3 scripts/measure-upload-resume.py` (`--quick` for a smoke run). It uses offset-varying data and prints its evidence directory.
