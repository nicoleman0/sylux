# Permanent removal and cleanup costs

Measured on 2026-11-18 with source `11d5141ea946160111021fcd3d31138dfcf50a73`.
The timed copy used `cargo build --locked --release --no-default-features --bin sylux`,
bundled SQLite 3.54.1, Rust 1.99.1 and a 10-core Apple Silicon Mac. It ran
with the normal writer and reader, WAL, synchronous FULL and foreign keys.
Timing probes ran in a disposable source copy. The benchmark used local
storage and a single client.

## Removal confirmation before the query fix

These baseline figures predate the tree-first query fix below.

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

## Tree-first query replay

On 2026-11-18, an instrumented release build of `b49ebe30` plus patch
SHA-256 `92b3df2b666874b835a611baca0da43e88501c46d5bf8703fea95933e6b5cb6a`
was measured on the same class of host. Each case ran five times, rotating
order, with fresh isolated roots and OS-assigned loopback ports. Each file had
one revision. Values are median [minimum–maximum] milliseconds.

| Selection | Unrelated live files | Preview fingerprint | Preview HTTP | Writer through commit | Execute HTTP |
| --- | ---: | ---: | ---: | ---: | ---: |
| One folder, 1,000 files | 0 | 1.96 [1.93–2.51] | 3.51 [3.44–10.63] | 13.60 [12.71–16.82] | 13.77 [12.89–17.04] |
| One folder, 1,000 files | 20,000 | 4.10 [4.01–4.26] | 5.63 [5.50–5.77] | 19.58 [19.19–20.02] | 19.82 [19.44–20.27] |
| All, 1 file | 20,000 | 2.04 [1.99–2.15] | 3.58 [3.51–3.72] | 4.21 [4.09–4.24] | 4.40 [4.26–4.44] |
| All, 100 files | 20,000 | 6.58 [6.50–6.74] | 8.11 [8.00–8.26] | 11.48 [11.29–11.74] | 11.69 [11.49–12.03] |

Preview runs the fingerprint on the single reader worker, so that worker is
occupied for at least the fingerprint duration shown. The writer fingerprint
for the 1,000-file folder with 20,000 live files was 3.75 ms [3.73–4.13].
The previous query on this fixture held the writer 3.93–4.30 s in three
review runs. The new plan walks the tree first and uses indexed parent,
entry and revision lookups. A separate SQL comparison returned identical
ordered rows for both queries on this fixture. The 10-second host monitor
sampled one-minute load from 5.52 to 10.06 on ten logical CPUs, including
other concurrent work; these measurements do not establish mixed-traffic
latency or results on another host.

To replay on macOS, build the web assets first and use a new evidence directory
outside the checkout. The scripts archive the selected revision, instrument a
disposable copy, and leave raw logs and fixtures in the evidence directory.
The build expects the current `web/dist` to match that revision. The revision
must contain the measured query fix.

```sh
export SYL89_EVIDENCE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/syl89-bench.XXXXXXXX")"
bash tools/benchmarks/syl-89-removal/build.sh "$PWD" HEAD
bash tools/benchmarks/syl-89-removal/run.sh review review --repeats 5
python3 tools/benchmarks/syl-89-removal/summarize.py "$SYL89_EVIDENCE_DIR/review.jsonl"
```

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

The original query plan scanned the owner's entries during recursive steps
and scanned entries and revisions at the final joins. The tree-first joins
address that measured bottleneck without changing the rows or ordering.
The idle runner and audit inputs did not change, so their earlier measurements
remain above. At 50,000 history rows the audit accounted for about 44% of
time to listen. The one-second cleanup fallback covers committed work whose
HTTP future drops before notification; scheduling and audit tuning remain
deferred.

These runs use warm filesystem caches, synthetic single-owner data and no
competing Sylux traffic. The earlier baseline sampled one-minute host load from 1.95 to
3.88 on ten logical CPUs. Cold storage, mixed traffic,
Linux x86_64, browser behavior and durability remain outside this evidence.
