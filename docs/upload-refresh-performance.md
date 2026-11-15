# Batch upload refreshes (SYL-60)

The UI fetches the destination's current descriptor from `GET /folders/{id}/path` before each file. It retains the captured destination ID across navigation and moves, and sends the current ETag through the existing ticket, admission and commit protocol. Server ownership, ancestry, session and precondition checks remain authoritative. A race after descriptor retrieval still fails admission or commit. There is no new retry or concurrency.

Confirmed files update transfer counts immediately. The existing batch finalizer refreshes root and current-view listings once after completion, cancellation, failure or an uncertain outcome. Standalone status reconciliation still revalidates listings.

## Measurements

Run on 15 November 2026 with the existing SYL-48 corrected runner:

```sh
PATH=/private/tmp/node-v24.22.2-darwin-arm64/bin:$PATH node scripts/measure-workflows.mjs --release --without-search
```

Both revisions use backend source `7a34bebc58ccfbd7ee396ea5da98859b3b885c83`, Rust 1.99.1, Node 24.22.2, Playwright 1.65.0 and installed Chrome 157.0.8103.51. The baseline embeds the original UI assets; the changed run embeds this PR's assets. Each run records the asset hash and executable attribution. Baseline asset compilation preceded UI edits; its later `source.diff` includes edits made while the compiled baseline ran. Asset hashes, rather than that diff alone, distinguish the UI under measurement.

Fresh disposable metadata fixtures use 10k/100k entries and loopback TLS on Apple M4/macOS. Each actual browser batch sends 60 files of 256 KiB to one folder and navigates to Files during upload. Timing includes final UI revalidation and two animation frames. One batch sample per revision/scale. Seeding and uploads warm caches; no OS cache flush. Browser regression tests and sibling search-baseline work shared the host during these runs. Timings describe these samples and do not establish an isolated speedup or supported capacity.

| Metadata entries | Before total / broad listings | After total / broad listings | Before completion | After completion |
| --- | --- | --- | --- | --- |
| 10,000 | 483 / 243 | 303 / 3 | 5,472.5 ms | 1,816.0 ms |
| 100,000 | 483 / 243 | 303 / 3 | 34,048.9 ms | 2,349.4 ms |

The three broad listings include one navigation request and two final revalidation requests. Both changed batches add 60 path reads; the 240 ticket/admission/content/commit requests remain unchanged. These samples show a deterministic request reduction; timing differences also include uncontrolled shared-host load.

Before: `2026-11-15T08-44-29.591Z-full`. After: `2026-11-15T08-48-34.883Z-full`. Both full runs exit nonzero for the separate 100k Trash browser crash after a completed upload; both fixtures joined and cleaned. Physical media for the temporary fixture path could not be resolved, so no storage-media attribution is made.

Search is deliberately excluded to separate refresh reduction from SYL-58. The existing broad benchmark also opens Trash after upload; its separate 100k browser crash remains SYL-59 work. An upload completion sample does not make the full run a pass.

Raw runs remain in ignored `reports/syl-48/`, including `samples.jsonl`, `resources.jsonl`, `run.json`, `source.diff` and harness snapshots. The adjacent JSON retains the upload samples, full progress events, attribution, lifecycle and hashes of raw files. It contains no credentials or fixture paths.

## UI comparison and verification

Comparison inspected 15 November 2026:

- [Nextcloud server issue 58352](https://github.com/nextcloud/server/issues/58352) reports upload destinations changing on navigation. Keep the captured folder ID and resolve its current descriptor. [Nextcloud 36's web interface manual](https://docs.nextcloud.com/server/latest/user_manual/en/files/access_webgui.html) supplies file-list and navigation screenshots. Retain Sylux's existing controls and quiet progress treatment.
- [File Browser issue 2573](https://github.com/filebrowser/filebrowser/issues/2573) reports missing refresh after upload. Retain automatic final revalidation. The report does not establish current behavior. No live comparison or upload-state screenshot was available for either product; this is documentation and issue inspection.

The browser regressions cover fresh ETags after each commit, moved nested destinations, removal, session expiry, a race rejected at admission, cancellation during destination lookup, replacements, uncertain commits, navigation, final pending feedback and completed-feedback clearing. Existing desktop/narrow and both-theme journeys exercise unchanged progress and result controls. Screenshots of desktop saving and narrow active progress were inspected. These are simulated transport tests, separate from the real TLS fixture measurements. Linux/container deployment, durability and intended-host qualification were not run.

Validation: Svelte check passed without diagnostics and production assets rebuilt. Installed Chrome ran 217/218 selected browser tests successfully; a sidebar expand-via-keyboard assertion failed once and passed independently on rerun. The two subsequently added cancel/moved-destination regressions passed separately. The initial default-browser invocation could not launch because the pinned Playwright browser was missing; it supplied no product evidence. No full Linux baseline check was claimed on this macOS host.
