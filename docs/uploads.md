# Large uploads and resume

Files can be up to 64 GiB. The browser sends one file at a time in sequential chunks of at most 8 MiB. Existing replacement confirmation and final destination checks still apply.

If a connection fails, the browser checks the original upload's status before retrying. After repeated failures, use **Check upload status**, then **Resume upload**. After a tab reload, sign in to the same account, check the upload and **Select original file**. The browser reads the file in bounded slices and verifies every byte against its retained identity before sending more data. A different file leaves the saved offset unchanged.

Reload recovery uses this tab's session storage. It requires that record to survive; a closed tab, cleared browser storage or another device cannot recover it. Signing out clears the account's record in the tab. No browser file handle or account credential is retained. The record contains the original signed upload ticket, admission and a file fingerprint. That ticket is usable only with the matching authenticated account and current CSRF token.

## Protocol

- `POST /mutation-tickets` returns the signed admission and the server's `max_upload_length` and `max_chunk_length`. The limits are defined in `src/domain/upload.rs`.
- `POST /uploads` admits the original mutation. Its ticket fixes the account, destination, name, length and any confirmed replacement.
- `GET /uploads/{id}` reports the acknowledged `bytes_received`, writer state and final outcome. The client uses this offset after an interruption, never its last progress event.
- `PATCH /uploads/{id}/content` appends a single chunk. Send `Content-Length`, `Content-Type: application/octet-stream`, `Upload-Offset` and `Upload-Checksum` (lowercase SHA-256 of this chunk), plus the original mutation ticket and normal authentication/CSRF headers. Missing length gets 411. Over-limit chunks get 413. Stale offsets or competing writers get 409 without advancing the offset. Incomplete or corrupt chunks get 422.
- Acknowledgement follows file sync and the SQL offset checkpoint. A request interrupted before acknowledgement can have either the old or the new offset. Read status before sending again. Unacknowledged trailing bytes are truncated at the recorded offset before reuse.
- The final chunk triggers one bounded streaming hash of the full file. Completed metadata is recorded before create-only payload publication, so restart reconciliation can finish an interrupted publication. The file remains invisible until `POST /uploads/{id}/commit` succeeds.
- Lost commit responses are resolved with status for the same mutation. A committed operation is never submitted again. A publication fault can report `recovery_required`; restart reconciliation must verify its candidate before commit is allowed.

The original full-body PUT route remains available. It refuses to restart an upload with an acknowledged nonzero offset; use PATCH to resume it. No chunk parallelism, cross-device resume or instant-upload deduplication is provided.

A replacement still requires the original target ETag and parent ETag at commit. If either changes, cancel and make a fresh, explicitly confirmed upload. Resume never silently changes the signed destination or replacement.

## Abandoned bytes

Tickets expire 24 hours after issuance. A bounded sweep starts when the service begins serving and runs every minute, visiting at most 32 eligible attempts per pass with a cursor. It rejects expired admitted work before unlinking its private staging file. It skips active writers, completed candidates and committed results; failed cleanup is retried on a later sweep. A large backlog can take multiple passes. Startup reconciliation clears interrupted writer state before serving.

Fully staged or uncertain publication candidates remain preserved for reconciliation. They are not exposed as files. They are deliberately outside partial-byte cleanup, which cannot determine that a completed candidate is safe to delete.

## Verification

- `cargo test --locked --features test-harness --test contract_uploads --no-fail-fast` covers offset conflicts, interruption, unacknowledged tails, account isolation, expiry, publication faults and the 64 GiB admission boundary.
- `npm --prefix web run smoke -- resumable-upload.spec.ts desktop-upload.spec.ts replacement.spec.ts` covers the client recovery record, wrong-file rejection, reload, cancellation and replacement UI with mocked APIs.
- `tests/browser/storage.spec.ts` adds the real TLS reload/reselect/resume journey to the ordinary feature-enabled browser fixture. Report that evidence separately from mocked UI tests and direct HTTP contracts.

Build current web assets, then run `python3 scripts/measure-upload-resume.py` to reproduce the disposable 4 GiB journey and paired throughput/RSS measurements. It streams downloads into a digest and stops/joins its owned fixture before cleaning payloads. Keep enough space for about 6 GiB of temporary payloads.

Local disposable verification on 17 November 2026, after integrating SYL-78, uploaded and streamed back 4 GiB with SHA-256 `8479e43911dc45e89f934fe48d01297e16f51d17aa561d4d1c216b1ae0fcddca`. A local TLS comparison using the Cargo test profile (optimized SHA-256 dependency) measured three 256 MiB PUT samples at 805.0/742.3/615.3 MiB/s and append at 235.0/299.6/308.9 MiB/s, with one persistent API connection. The interrupted 4 GiB append run reached 262.9 MiB/s and 60.9 MiB maximum sampled server RSS. The 256 MiB append samples peaked at 60.5 MiB. These are functional development measurements on a shared host, not release throughput or physical durability qualification. Append pays for each chunk's checksum, file sync, SQL checkpoint and a final full-file verification pass. Browser identity checking is a separate full file read before transmission and again on reselect.

## UI comparison

Inspected 17 November 2026. Primary behavior documentation was read before implementation; the published screenshot comparison was completed during verification. No live Nextcloud or copyparty instance was available.

- [Nextcloud 35 chunk API](https://docs.nextcloud.com/server/stable/developer_manual/client_apis/WebDAV/chunking.html) and its [Files screenshot](https://docs.nextcloud.com/server/stable/user_manual/_images/users-files.png): adopt private temporary chunks and familiar filename/list context. Sylux uses one append file instead of a separate assembly step, and retains its existing quiet controls.
- [copyparty's current README, uploading section](https://github.com/9001/copyparty#uploading) and its linked [simplified uploader screenshot](https://user-images.githubusercontent.com/241032/118311195-dd6ca380-b4ef-11eb-86f3-75a3ff2e1332.png): adopt distinct checking/sending stages, progress and explicit reselect after browser loss. Reject the parallel-upload selector and advanced controls for this sequential workflow. The screenshot is a published example linked by the README, not a fresh capture of the current release.
