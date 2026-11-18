# Permanent removal and cleanup costs

Measured on 2026-11-18 with source `11d5141ea946160111021fcd3d31138dfcf50a73`.
The timed copy used `cargo build --locked --release --no-default-features --bin sylux`,
bundled SQLite 3.54.1, Rust 1.99.1 and a 10-core Apple Silicon Mac. It ran
with the normal writer and reader, WAL, synchronous FULL and foreign keys.
Timing probes ran in a disposable source copy. The benchmark used local
storage and a single client.

## Removal confirmation

Each file had one revision and one verified empty payload. The folder case
had 1,000 active children beneath one trashed root. Other cases used flat
trashed files. A fresh fixture copy served each request. The client requested
a preview, then posted the returned confirmation on the same loopback TLS
connection. It verified the status, operation ID, removed-root count and
persisted membership and cleanup rows.

| Selection | Fingerprint inside writer transaction (ms) | Writer submission through commit (ms) | Execute HTTP (ms) |
| --- | ---: | ---: | ---: |
| All, 1 file | 0.10 [0.09–0.11] | 0.57 [0.56–0.92] | 0.72 [0.70–1.09] |
| Selected, 100 files | 6.25 [6.03–6.67] | 9.18 [9.05–10.00] | 9.39 [9.24–10.26] |
| All, 100 files | 6.15 [5.85–6.44] | 8.93 [8.48–10.62] | 9.11 [8.66–10.80] |
| All, 1,000 files | 140.16 [139.10–140.92] | 165.41 [164.39–169.08] | 165.66 [164.60–169.33] |
| All, one folder with 1,000 files | 156.85 [152.66–158.36] | 166.52 [162.35–168.43] | 166.75 [162.57–168.66] |

Values are medians and observed ranges over five rotated runs per size.
Rust's monotonic clock measured the fingerprint call in `removal::execute`
and the awaited writer submission in `http::removals::execute`. The client
used its monotonic clock through response parsing. Writer time includes
dispatch, authorization and commit. HTTP includes neither preview nor
post-commit cleanup.

Three separate runs at 10,000 flat roots produced a median fingerprint
time of 12.37 s [12.10–12.85] and execute HTTP time of 12.75 s
[12.45–13.24]. The writer remained occupied for essentially the full
request.

## Idle runner and startup audit

History fixtures used one removed revision per blob, authorized cleanup rows
marked complete and no live payloads. Each startup used a fresh process.
Rust timed the history audit read in `reconcile`, including the inconsistency
query and per-blob authorization checks. The client timed process start
through the listener event. It checked readiness and verified that adding an
unauthorized non-live blob refused startup before the listener.

| Completed cleanup rows | Audit (ms) | Reconciliation (ms) | Start to listener (ms) |
| --- | ---: | ---: | ---: |
| 0 | 0.024 [0.023–0.030] | 0.54 [0.52–0.58] | 17.29 [16.93–19.70] |
| 1,000 | 6.41 [6.39–6.58] | 9.69 [9.65–10.03] | 30.95 [29.67–61.15] |
| 10,000 | 70.13 [69.82–72.54] | 108.40 [107.46–113.68] | 178.15 [173.17–290.25] |
| 50,000 | 441.03 [436.72–445.44] | 691.84 [684.92–707.02] | 1,009.89 [999.49–1,034.09] |

Each startup size ran five times. With no notification or pending work, five
6.3-second idle windows per size yielded 30 polls each at zero and 50,000
history rows. Median whole-pass elapsed time was 242 µs [72–323] and 260 µs
[82–362], respectively, at roughly one pass per second. The SQLite reader
portion was 158 and 168 µs median. The pending lookup used the covering
`removal_cleanup_pending` index. These durations describe elapsed reader
and scheduling work, rather than process CPU utilization.

## Disposition and limits

Separate CPU samples showed SQLite statement execution in the fingerprint
and SQLite prepare work in the history audit. Timing the three statements
prepared per root in the 10,000-root diagnostic attributed about 0.43 s
of 12.38 s to preparation. Caching those statements would address a small
part of the stress result. At 50,000 history rows the audit accounted for
about 44% of time to listen. The one-second cleanup fallback had a small
observed cost and covers committed work whose HTTP future drops before
notification. SYL-89 records this baseline and defers query or scheduling
tuning.

These runs use warm filesystem caches, synthetic single-owner data and no
competing Sylux traffic. Sampled one-minute host load ranged from 1.95 to
3.88 during timed runs on ten logical CPUs. Cold storage, mixed traffic,
Linux x86_64, browser behavior and durability remain outside this evidence.
