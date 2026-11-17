# Large uploads and resume

Files can be up to 64 GiB. The browser sends one file at a time in sequential chunks of at most 8 MiB. Existing replacement confirmation and final destination checks still apply.

If a connection fails, the browser checks the original upload's status before retrying. After repeated failures, use **Check upload status**, then **Resume upload**. After a tab reload, sign in to the same account, check the upload and **Select original file**. The browser reads the file in bounded slices and verifies every byte against its retained identity before sending more data. A different file leaves the saved offset unchanged.

Reload recovery uses this tab's session storage. It requires that record to survive; a closed tab, cleared browser storage or another device cannot recover it. Signing out clears the account's record in the tab. No browser file handle or account credential is retained. The record contains the original signed upload ticket, admission and a file fingerprint. That ticket is usable only with the matching authenticated account and current CSRF token.

## Protocol

- `POST /mutation-tickets` returns the signed admission and the server's `max_upload_length` and `max_chunk_length`. The limits are defined in `src/domain/upload.rs`.
- `POST /uploads` admits the original mutation. Its ticket fixes the account, destination, name, length and any confirmed replacement.
- `GET /uploads/{id}` reports the acknowledged `bytes_received`, writer state and final outcome. The client uses this offset after an interruption, never its last progress event.
- `PATCH /uploads/{id}/content` appends a single chunk. Send `Content-Length`, `Content-Type: application/octet-stream`, `Upload-Offset` and `Upload-Checksum` (lowercase SHA-256 of this chunk), plus the original mutation ticket and normal authentication/CSRF headers. Missing length gets 411. Over-limit chunks get 413. Stale offsets or competing writers get 409 without advancing the offset. Incomplete or corrupt chunks get 422. A canceled upload gets 409 `upload_canceled`. A stalled body gets 408 `chunk_idle_timeout` and a stopping server 503 `server_stopping`; both leave the offset unchanged, so read status and resume.
- Acknowledgement follows file sync and the SQL offset checkpoint. A request interrupted before acknowledgement can have either the old or the new offset. Read status before sending again. Unacknowledged trailing bytes are truncated at the recorded offset before reuse.
- The final chunk triggers one bounded streaming hash of the full file. Completed metadata is recorded before create-only payload publication, so restart reconciliation can finish an interrupted publication. The file remains invisible until `POST /uploads/{id}/commit` succeeds.
- A chunk body that stalls for 30 seconds between frames is abandoned with 408. The limit applies per frame, not to the whole chunk, so slow steady links are unaffected. The server releases the writer without advancing the offset, so status shows `writer_active: false` and the client resumes from the same offset.
- Lost commit responses are resolved with status for the same mutation. Commit applies at most once: a concurrent or later repeat of the same mutation gets 409 `upload_not_staged` and has no effect. After an uncertain commit the tab checks status first, then finishes a still-`publishing` upload with the same digest. A publication fault can report `recovery_required`; restart reconciliation must verify its candidate before commit is allowed.

The original full-body PUT route remains available. It refuses to restart an upload with an acknowledged nonzero offset; use PATCH to resume it. No chunk parallelism, cross-device resume or instant-upload deduplication is provided.

A replacement still requires the original target ETag and parent ETag at commit. If either changes, cancel and make a fresh, explicitly confirmed upload. Resume never silently changes the signed destination or replacement.

## Time limit

A ticket expires 24 hours after issuance, in absolute time. Progress does not extend it. A 64 GiB upload therefore needs an average of at least about 0.76 MiB/s (6.4 Mbit/s) across the whole window, including pauses, reloads and the browser's full-file identity read. Slower links cannot finish a file of that size.

The browser refuses to send, resume or commit once the retained ticket's expiry has passed, before reading the file again. The user checks the status and starts a new upload after the old attempt is resolved.

## Abandoned bytes

A bounded sweep starts when the service begins serving and runs every minute, visiting at most 32 eligible attempts per pass with a cursor. It rejects expired admitted work, unlinks its private partial staging file, then records a durable `partial-cleaned` marker on the attempt so later passes skip it. Failed cleanup is retried on a later sweep. A large backlog can take multiple passes. Startup reconciliation clears interrupted writer state before serving.

What is kept:

- Active writers, committed results and completed candidates are outside partial cleanup.
- A fully staged or uncertain publication candidate stays for reconciliation and is not exposed as a file.
- A staged upload that is later rejected, for example canceled before commit, keeps its staged payload. No cleanup rule exists for it yet.

## Verification

- `cargo test --locked --features test-harness --test contract_uploads --no-fail-fast` covers offset conflicts, interruption, unacknowledged tails, account isolation, expiry, cleanup, publication faults and the 64 GiB admission boundary.
- `npm --prefix web run smoke -- resumable-upload.spec.ts desktop-upload.spec.ts replacement.spec.ts` covers the client recovery record, wrong-file rejection, reload, cancellation and replacement UI with mocked APIs.
- `tests/browser/storage.spec.ts` adds the real TLS reload/reselect/resume journey to the feature-enabled browser fixture. Report that evidence separately from mocked UI tests and direct HTTP contracts.
- `python3 scripts/measure-upload-resume.py` runs a disposable TLS journey: paired PUT and append throughput, an interrupted 4 GiB append with a byte-exact streamed download, and three concurrent distinct uploads sampling server RSS. Source data is deterministic and varies by offset, so a misplaced, replayed or reordered chunk changes the digest. Use `--quick` for a small smoke run, which is not large-file evidence. Build current web assets first and keep about 6 GiB free. Results and exact build metadata are written to a temporary directory the script prints. Its owned fixture and payloads are removed on exit.
