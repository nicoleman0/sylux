# Storage use

Account shows the signed-in user's Files and Trash totals. Settings > Accounts shows both totals for every account and the free space on the data volume. Only administrators can read that view.

## Definitions

Figures are logical sizes: the sum of stored revision lengths. They exclude SQLite metadata, filesystem block overhead, upload staging and payloads still waiting for removal cleanup. They are not disk use.

- **Files**: revisions of files reachable from any of the account's library roots through active folders.
- **Trash**: revisions of every other file that has not been permanently removed. This includes the contents of trashed folders and files replaced by uploads.

Every retained revision falls in exactly one category. A replacement upload keeps the previous file as a deleted entry, so its bytes move from Files to Trash. Sylux records no provenance for that entry, so replaced files are not reported separately. Permanent removal takes bytes out of Trash when it commits, before payload cleanup finishes.

A category containing a file without a content revision reports `null`. The UI shows Unknown for that, for a failed request and for an unreadable volume, and Loading… while a request is pending. None of these display as zero.

Free space is `f_bavail × f_frsize` from `fstatvfs` on the payload directory: the bytes an unprivileged process can still allocate there.

## API

- `GET /me/usage` returns `{files_bytes, trash_bytes}` for the session's account.
- `GET /accounts/usage` requires an administrator. It returns `{volume: {available_bytes, total_bytes} | null, accounts: [{account_id, files_bytes, trash_bytes}]}`.

Both responses are `no-store`.

## Measurement

The opt-in metadata fixture accepts a `usage` command. It checks results against the seed oracle and records sequential closure, HTTP and `fstatvfs` timings:

```sh
cargo test --locked --release --features test-harness --test performance_fixture --no-run
printf 'usage\nusage\nprobes-fast\nstop\n' | <performance_fixture executable> --run 100000
```

At 100,000 entries on the Apple M4 host described in [performance.md](performance.md), usage took a median of about 180 ms per account, for both the closure and HTTP. That is within the 161–273 ms range for 100k account-wide searches. Folder listings read one page and are not a like-for-like baseline. Each request walks the whole account once.
