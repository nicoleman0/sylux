# File workflow measurements

SYL-48 measures existing behavior. It adds no serving instrumentation, schema changes or latency gates. The fixture is gated by `test-harness`; ordinary Cargo runs, filters and discovery print a skip message. Only an explicit first `--run` argument activates the fixture; the Node runner supplies it.

## Reproduce

Use the versions in `.nvmrc`, `web/package.json` and `rust-toolchain.toml`. Install web dependencies on a fresh checkout. Chrome and OpenSSL must already be available. This runner uses Playwright from `web/node_modules` and an isolated installed Chrome process.

```sh
npm --prefix web run check
npm --prefix web run build
cargo test --locked --release --features test-harness --test performance_fixture --no-run
node scripts/measure-workflows.mjs --quick --release
node scripts/measure-workflows.mjs --release
# Explicit recovery and bounded diagnostic after a slow baseline:
node scripts/measure-workflows.mjs --release --without-search
node scripts/measure-workflows.mjs --release --search-only
node scripts/measure-workflows.mjs --release --search-matrix
node scripts/measure-workflows.mjs --release --trash-only
node scripts/measure-workflows.mjs --release --trash-only --scale=100020
node scripts/measure-workflows.mjs --release --scale=10000 --transfers-only
```

The measurement Mac used the pinned Node installation at the front of `PATH`. Browser launch and `ps` sampling need permission outside the execution sandbox. No host trust store is changed. Node trusts only the generated CA; Chrome's certificate exception is restricted to the generated leaf's SPKI. The server listens only on loopback.

The runner builds the fixture through Cargo JSON messages, uses Cargo's returned executable, and records its hash and actual profile. Startup checks embedded fixture/assets hashes and the source SHA against current inputs.

Each run creates an ignored `reports/syl-48/<timestamp>-<mode>/` directory:

- `run.json`: command, source SHA, harness hashes, asset index hash, profile, versions, machine, fixture dimensions, assumptions, exit status and joined cleanup evidence.
- `samples.jsonl`: raw HTTP samples, browser timings, progress events, batch request counts and separate queue/service probes.
- `resources.jsonl`: sampled process CPU percentage and RSS. CPU is the operating system's process percentage, not an interval utilization trace. Browser descendants are grouped separately from the backend and runner.
- `summary.json`: nearest-rank percentiles grouped by scale, sample type and operation. Sparse p95/p99 values are exploratory maxima, not stable tail estimates.
- `source.diff` and `harness-snapshot/`: tracked diff and exact copies of the harness sources, including untracked files, as executed. Hashes identify those copies before commit.

`--scale=10000` or `--scale=100000` restricts a run to one scale. `--without-search` explicitly excludes account search, typing, search during transfer and search queue probes. `--search-only` runs deepest-folder metadata cases and one bounded no-match HTTP search per scale. `--transfers-only` runs API batches and one mixed probe batch. `--search-matrix` runs seven HTTP samples for each search shape, each paired with an overlapping name-ascending browse request, then three batches of the existing twelve mixed queue/service probes. Both responses are checked against the seed oracle. HTTP dispatch does not impose reader admission order. This mode excludes browser journeys, transfers, content and standalone browse cases. These limited modes never establish full-matrix acceptance.

A failed run exits nonzero and retains partial evidence. Preview and main browser phase failures are recorded and the API transfer/probe phases continue. SIGINT, SIGTERM and the overall deadline cancel active work. One idempotent stop operation closes stdin and bounds child join/kill; interrupted runs retain nonzero status and evidence. The runner closes browser contexts and destroys requests before stopping the fixture. A forced termination retains its disposable fixture. Only the fixture owner removes its own marked root after the TLS server and SQLite workers join. No user data is read or deleted.

## Workloads and interpretation

- Full mode uses fresh 10,000 and 100,000 entry fixtures. Quick mode uses 1,000 entries, fewer repetitions and 4/8 MiB content. Quick results verify tooling only.
- Metadata seed 48 creates a 16-folder chain, a wide root and files spread across deep parents. It adds ten upload folders and one account root. File lengths use `(index * 7919 + 48) % 16777216`; modified times use `1700000000 + index % 86400`. Every tenth file is deleted. Exact counts are recorded for each fixture.
- Metadata fixtures contain valid entries, revisions and blob metadata but no payload files. Browse, search and Trash may use them. Content requests target only files committed through the upload API. This benchmark is not an integrity or reconciliation test.
- HTTP cases cover all six sorts, up to page ten, selective/broad/no-match/one-character searches and Trash. The full baseline repeats each primary case seven times. Late pages require preceding cursor requests; raw samples preserve each page number. Deepest-folder cases cover a 17-entry ancestor path and all six listing sorts. Seed dimensions include live root children and siblings at each deep layer.
- The shipped browser UI performs all six sorts, paging, the search shapes, debounced typing and Trash. Corrected timings match the exact requested search query, wait for the Search results view to finish and match response rows, then wait two animation frames. Other actions wait for response/loading completion and two frames. They are a completion-to-paint proxy, not compositor presentation telemetry. Browser cases have one sample each.
- The synthetic UI batch uploads 60 files of 256 KiB each (64 KiB in quick mode) into `uploads-00`, while browsing and searching elsewhere. Timing includes final UI completion after the refresh. XHR progress events and browser request counts are retained. This is not an exact reproduction of a reported incident whose original file sizes and deployed version are unknown.
- Separate synthetic API batches upload 60 files of 1 MiB each (64 KiB quick), distributed across 1, 5 and 10 distinct valid sessions of the same account. Each session writes to its own folder. The runner reads a parent listing and path descriptor before admission, requests a ticket, admits, streams and commits each file. It polls status during streaming and runs browsing and one selective search while the batch is active. Browsing pauses 50 ms between observations. Each 64 KiB body chunk is paced by 2 ms; this synthetic pacing provides an observation window. Request counts include this observer traffic and sample byte verification. In-stream status reports returned zero bytes in these samples; the backend persists the completed length at staging. XHR events measure actual browser transport progress, not durable server progress. Corrected runs record upload-worker completion, total workload wall time, observer wall time and observer tail separately. Historical API values below are workload wall times including observer tail; upload-worker completion was not captured.
- Large real uploads are 256 MiB and 1 GiB of deterministic `a` bytes. Each receives three HEAD/conditional GET/4 KiB range/full download sequences. Full downloads validate byte count and SHA-256 while streaming. Browser previews exercise the actual 2 MiB text-preview size fallback.
- Queue probes submit twelve concurrent closures to the real `SqliteWorkers` reader and call production listing, search and Trash functions. Recorded wait includes dispatch, admission and scheduling until the closure starts; service runs inside that closure. Three batches run after API transfers in full mode; the narrow transfer pass uses one batch. These counts are recorded and are too sparse for stable tail estimates. These probes are not request latency and do not measure HTTP-induced queue occupancy.
- Fresh processes and fixtures do not establish disk-cold behavior. Seeding warms metadata; uploading warms content. The first content sequence means the first request after upload. Warm samples reuse process and connection state. No OS cache flush, network emulation, physical LAN, remote clients or production storage were used. Content responses use the product's `no-store` policy.
- The fixture uses four Tokio executor threads plus its SQLite workers. `ttfbMs` is request start to the response-headers callback, a headers-arrival proxy rather than a directly observed first body byte.
- Filesystem statistics come from the live owned fixture path. Media fields remain unknown if that path cannot be resolved by the platform tool. Historical root-volume observations do not prove the temporary fixture used the same backing storage.
- CPU/RSS sampling adds `ps` overhead once per second. Resource snapshots show coincident load; they do not attribute CPU to a particular SQL statement. The runner also hashes download streams, so runner CPU is not backend CPU.

## Historical full-run findings

Local source: `3177f27d8efb26d8bd944867c164902238e1897d`, release profile, Rust 1.99.1, Node 24.22.2, Playwright 1.65.0, installed Chrome 157.0.8103.51. Host: Apple M4, 10 logical CPUs, 16 GiB RAM, Darwin 25.6.0 arm64. APFS, solid-state and Apple Fabric were observed for the root volume; the historical fixture backing volume was not independently verified. These are loopback TLS results. Historical browser action timings used the earlier response/loading/two-frame proxy, which did not match the exact search query or wait for Search results completion. Historical HTTP validation checked status/JSON but lacked the corrected seed oracle. These runs remain diagnostic evidence, not corrected-harness acceptance.

The baseline run `2026-11-14T23-22-25.013Z-full` failed at the 10k UI batch's 180-second deadline after concurrent search began. It retained 162 samples. It did not reach 100k, API batches, queue probes or large content. Seven HTTP samples per baseline operation show:

| Metadata scale | Name ascending median | Size descending median | Modified descending median | Selective search median | No-match search median |
| --- | ---: | ---: | ---: | ---: | ---: |
| 10k | 6.21 ms | 6.93 ms | 9.69 ms | 4,192 ms | 4,177 ms |

Broad and one-character search medians were 4,113 ms and 4,174 ms. A concurrent UI search took 4,216 ms after upload started. Browser upload completion at this scale is a failed measurement, not a passing batch duration. The exact original incident workload is unknown.

Recovery run `2026-11-14T23-28-41.576Z-full` explicitly omitted search. The 10k UI batch completed after its final refresh in 4,750 ms, with 483 browser requests: 121 root listings, 122 folder listings and 60 requests each for ticket, admission, body and commit. Each item was 256 KiB. Separate 60-file, 1 MiB API workload wall times were 2,895/885/482 ms with 1/5/10 sessions; request counts were 469/436/431 including observer and verification requests. These are single batch samples with transport pacing, not concurrency capacity estimates.

At 100k, the same recovery run measured name ascending, size descending and modified descending HTTP medians of 74.12/74.39/105.74 ms. Chrome then reported `page.waitForFunction: Target crashed` while opening Trash with 9,998 deleted entries. Its aggregate browser/sampler descendant RSS peaked near 3.8 GiB in one-second samples. No heap, crash dump or OOM telemetry was captured, so the cause of the crash is unknown. The failed browser phase prevented that older runner version from reaching 100k API batches and probes. A focused recovery pass uses the revised runner's isolated browser phase.

Both scales completed real large uploads, conditional/range requests, full SHA-256 verification and the browser size fallback before that crash. In the recovery run's 100k fixture, the 256 MiB warm download samples delivered 356/357 MiB/s with 112/113 ms TTFB; 1 GiB warm samples delivered 356/362 MiB/s with 445/449 ms TTFB. Warm HEAD/304/range operations still took roughly 112–116 ms at 256 MiB and 453–463 ms at 1 GiB. These read paths have material size-dependent work even with warm caches. Any follow-up must preserve payload verification boundaries.

The actual Rust SQLite plan reports `SEARCH child USING INDEX entry_active_listing (owner_account_id=?)` followed by `SCAN live` in the recursive step. It does not constrain `parent_id` in that lookup. This supports a focused search-query-plan follow-up. It does not establish a production speedup for an unshipped SQL change. No production SQL was changed.

The focused 100k recovery `2026-11-14T23-31-45.314Z-full` moved Trash after the upload journey and isolated browser failures. Its UI batch completed in 23,849 ms with the same 483-request breakdown. Trash reproduced `page.waitForFunction: Target crashed`; the runner then completed API batches and probes and joined/cleaned the fixture. API workload wall times were 9,766/2,055/2,261 ms for 1/5/10 sessions, with 483/436/435 total requests. Search was excluded throughout this recovery pass.

At 100k, nine samples each from three fast probe batches gave median closure service of 72.9 ms (name), 74.2 ms (size), 106.6 ms (modified) and 75.8 ms (Trash). Waiting reached 913 ms across the deliberately simultaneous twelve-closure submissions. This measures a controlled backlog, not queue wait observed in the HTTP workload.

The deep/search diagnostic `2026-11-14T23-34-02.986Z-full` passed all six deepest-folder sorts and the 17-entry ancestry path at both scales. Root live child counts were 7,197/71,997; deepest live siblings were 125/1,250; Trash counts were 998/9,998. At 100k, deepest-folder listings took about 10–11 ms in the focused recovery; ancestry took 0.83 ms. The separate 100k no-match request reached its 120,003 ms deadline. That is a timeout, not its completion latency. The fixture's active worker could not drain within the runner's shutdown bound; the process was killed and its marked disposable root was retained. No HTTP search at 100k is reported as passed.

The narrow transfer run `2026-11-14T23-37-16.867Z-full` passed at 10k. Each 60-file batch launched exactly one selective search while uploads were active, with 50 ms between other browse observations. Search samples were 4,197/4,567/4,343 ms for 1/5/10 sessions; API workload wall times were 7,091/5,450/4,939 ms. One twelve-closure mixed probe batch recorded search service of 4,380–4,554 ms and queue wait reaching 17,924 ms. These are one request per concurrency level and one deliberately queued probe batch, not stable percentiles or production queue occupancy.

| Required area | 10k evidence | 100k evidence |
| --- | --- | --- |
| Wide/deep listings, all sorts, paging/path | Passed | Passed |
| API/browser search shapes and repeated typing | Passed individually | No-match timed out; other shapes and typing not run |
| Trash HTTP/browser | Passed | HTTP passed; browser crashed |
| Real 60-file UI upload and progress | Completed without search; search-active baseline exceeded deadline | Completed without search |
| 1/5/10-session API uploads and browse/search | Passed; one overlapping search each | Upload/browse passed; overlapping search omitted after baseline timeout |
| Queue/service probes | Fast and mixed probes measured | Fast probes measured; search probes omitted after baseline timeout |
| 256 MiB/1 GiB real content, hashes and preview fallback | Passed | Passed |

Final quick full-workflow regression `2026-11-14T23-36-40.756Z-quick` passed with 2,136 samples. It verifies tooling at 1k and 4/8 MiB; it does not fill the full-scale omissions above. The full matrix is not a passing acceptance gate. Failed and omitted dimensions are useful measurements that constrain the next work.

Ranked follow-up work:

1. [SYL-58: search query plan](https://linear.app/corvus-dev/issue/SYL-58). The 10k search medians and 100k timeout justify investigating the recursive lookup plan. Preserve owner scope, ancestry, ordering and cursor-fingerprint behavior. An isolated SQL experiment is distinct from shipped HTTP evidence.
2. [SYL-59: bounded Trash](https://linear.app/corvus-dev/issue/SYL-59). The API returned all 9,998 deleted rows, while the browser crashed during rendering. Bound server and browser work while preserving recovery context and permissions. Crash causation remains unproved.
3. [SYL-60: upload refresh amplification](https://linear.app/corvus-dev/issue/SYL-60). Both completed UI batches issued 243 metadata listings around 240 ticket/admission/body/commit requests. Investigate repeated parent/root refreshes before raising concurrency. Preserve fresh parent ETags, explicit uncertain outcomes and reconciliation.
4. Warm large-content request cost. HEAD, 304 and 4 KiB range cost rose with real payload size despite warm caches. Profile the checked content-open path and preserve byte-integrity checks before choosing an optimization. No disk-cold or intended-host result is available.

SYL-58, SYL-59 and SYL-60 remain in Backlog. The owner chose to finish measurements and defer product fixes.

## Corrected harness verification

Corrected quick run `2026-11-14T23-51-27.035Z-quick` passed the 1k/4–8 MiB journey with deterministic seed checks for membership, sort order, pagination advancement, exact search counts, Trash and deep ancestry. It also used exact query matching and completed Search results before the paint proxy. It did not rerun or replace the historical 10k/100k measurements.

Corrected interruption checks `2026-11-14T23-52-54.942Z-quick` and `2026-11-14T23-52-55.116Z-quick` exited nonzero with `interrupted` status during Cargo bootstrap and one active TLS request respectively. The active-request fixture joined and cleaned. The small retained Node suite tests the actual sanitizer, seed validators, option parser and idempotent child stop; negative validator checks reject empty, wrong, repeated and nonadvancing data.

An injected first-preview launch failure in `2026-11-14T23-55-34.293Z-quick` produced a recorded preview failure and final nonzero status, while the next content size, main browser journey, all three API batches and probes continued; the fixture joined and cleaned. This is artificial failure-path verification, not a product failure or performance result. The injection command and assertions are retained in `corrected-preview-failure-check.json`.

After the final child observer was changed to wait for stdio `close`, quick search-only run `2026-11-14T23-57-20.933Z-quick` passed with 14 samples and clean fixture join. Final formatting, release all-target Clippy, numeric/discovery skip checks and all seven retained helper tests passed.

The final child-output drain check `2026-11-14T23-57-20.933Z-quick` passed the short TLS/deep/search path with clean shutdown. Seven retained helper test groups passed. CI runs these unit checks and Cargo discovery; it does not execute the benchmarks.

## Recursive search traversal correction (SYL-58)

The recursive step now starts with `live CROSS JOIN entry child`. SQLite documents that [CROSS JOIN prevents join reordering](https://www.sqlite.org/lang_select.html#special_handling_of_cross_join). The bundled SQLite 3.54.1 plan at both scales changes from `SEARCH child USING INDEX entry_active_listing (owner_account_id=?)` to `SEARCH child USING INDEX entry_active_listing (owner_account_id=? AND parent_id=? AND state=?)`. No schema, matching, ordering, fingerprint or cursor rule changed.

The focused release runs used base `7a34bebc58ccfbd7ee396ea5da98859b3b885c83` plus their recorded diffs, Rust 1.99.1, Node 24.22.2 and the same Apple M4/Darwin arm64 host and seed described above. [Portable raw samples and run metadata](measurements/syl-58-search.jsonl) retain executable, harness and raw-sample hashes, plans, dimensions, fixture filesystem observations and cleanup outcomes. Full local source snapshots, diffs and resource traces remain under ignored `reports/syl-48/<runId>/`.

| Search HTTP median | 10k before, ms | 10k after, ms | 100k after, ms |
| --- | ---: | ---: | ---: |
| selective | 4,818.92 | 12.62 | 165.64 |
| broad | 5,757.72 | 24.00 | 278.63 |
| none | 5,486.54 | 17.83 | 164.87 |
| one | 5,368.26 | 28.89 | 271.96 |

Each cell has seven completed samples. Each search was paired with an overlapping first-page name browse, rather than the standalone searches in the historical full matrix. Final after samples include pair IDs and monotonic HTTP start/end times. These intervals measure outstanding HTTP requests; they do not prove SQLite admission order. At 100k before, the first selective search and paired browse both exceeded the 120-second HTTP bound. No baseline 100k result, other search shapes or queue probes completed. That failure required killing the fixture, which retained its disposable data; all 10k-before and both-scale after fixtures joined and cleaned normally.

| Search closure service median | 10k before, ms | 10k after, ms | 100k after, ms |
| --- | ---: | ---: | ---: |
| search_selective | 6,326.28 | 12.20 | 161.15 |
| search_broad | 8,430.99 | 23.10 | 272.25 |
| search_none | 7,713.68 | 12.41 | 162.11 |
| search_one | 5,926.21 | 22.93 | 272.93 |

Queue evidence is separate from HTTP latency: each scale/pass has three twelve-closure batches. Selective search has six service samples; each other search shape has three. Closure service excludes admission wait and HTTP serialization. Across the 36 mixed probes, median/max queue wait was 21,085/34,569 ms before at 10k, 58/124 ms after at 10k and 774/1,475 ms after at 100k. Paired-browse timings and every individual queue/service sample are retained in the linked raw record.

The before 10k run overlapped other benchmark fixtures, including the bounded 100k baseline, and local compilation. The after run used a quiet benchmark window. These descriptive timings are not a controlled speedup ratio. Seeding warmed metadata; no OS cache eviction or physical storage qualification occurred. Fixture-path filesystem type was observed, but physical media metadata was unavailable. Sparse maxima do not establish tails or capacity. This focused pass contains no browser, transfer, deployment, durability or upgrade evidence.

Regression checks compare complete ordered traversal inputs before/after on a wide tree and a depth 128 chain, including case-equivalent duplicate file names, Unicode, literal wildcards, other accounts, deleted ancestors and the depth cutoff. Rename, generation, move and deletion preserve query equivalence while changing fingerprints. Plan checks use fresh 10k/100k mixed wide/deep data before and after `ANALYZE`. All 70 library and 41 names/listing integration tests passed, including real TLS search pagination and stale-cursor rejection; the expanded two search regressions passed again after review. Release all-target Clippy and seven measurement-helper groups passed. Web source and dependency inputs were unchanged; current built assets were reused before Cargo.

## Bounded Trash (SYL-59)

`GET /trash` now returns at most 200 rows (100 by default), with signed account/state cursors and deterministic original-name/ID ordering. Returned paths stay page-bounded; same-name peers are streamed one ancestry at a time to disambiguate historical locations. The UI renders one 100-row page at a time. Namespace fingerprinting streams account rows; SQL scanning/sorting still scales with account size. This change bounds retained response/path objects and DOM rows, rather than making database cost independent of inventory size.

`--trash-only` traverses every API page against the seed oracle, then opens Trash and uses Next/Previous through the actual TLS UI. It records response bytes, request counts, exact rendered-name matching, loading completion and two animation frames, plus one-second process CPU/RSS samples. It excludes search, uploads, content and queue probes. Full-mode Trash HTTP samples now mean the first bounded page. `--scale=100020` extends the same seed to exactly 10,000 deleted rows; it does not alter the production schema or seed deletion rule.

Final runs on 15 November 2026 used release Rust 1.99.1, Node 24.22.2, Playwright 1.65.0 and installed Chrome 157.0.8103.51 on Apple M4/Darwin arm64, loopback TLS. [Portable raw samples, resource observations and attribution](measurements/syl-59-trash.jsonl) retain the exact assets/harness/diff hashes and fixture cleanup. Source was `2cd32f7124827c586523cd5164dd895944301a37` plus the recorded SYL-59 diff. Original scales: `2026-11-15T10-30-26.203Z-full`; exact-10k-deleted extension: `2026-11-15T10-30-43.486Z-full`.

| Metadata / deleted entries | API pages traversed | UI rows per page | First / Next / Previous completion |
| --- | ---: | ---: | --- |
| 10,000 / 998 | 10 | 100 | 103.6 / 91.1 / 99.5 ms |
| 100,000 / 9,998 | 100 | 100 | 173.4 / 326.4 / 165.4 ms |
| 100,020 / 10,000 | 100 | 100 | 454.6 / 161.7 / 164.6 ms |

Each UI action made one Trash request and returned 21,343 bytes in these first/second-page samples. One sample per action/scale, after API traversal warmed metadata. The full API traversal checked every seeded deleted ID in order, including the short last page. Both final runs passed and every fixture joined and cleaned. The original unbounded UI crash was reproduced earlier on the unchanged Trash code in `2026-11-15T08-48-34.883Z-full`; that run's 100k failure and earlier complete-response HTTP samples are retained in the linked raw record. It used the broader upload/content workload and different source/assets, so timings and resource maxima are not controlled before/after ratios.

One-second descendant RSS maxima were about 867 MiB across the two-scale final run and 821 MiB in the extension. These aggregate Chrome/sampler descendants, include full-page screenshots and are not browser heap measurements. CPU/RSS observations are retained without attributing the earlier crash to OOM or a specific component. Concurrent browser regressions shared the host during part of this work. Seeding and traversal warmed metadata; no cache eviction, network emulation or physical-device/storage qualification occurred. Fixture-path physical media metadata was unavailable. Functional success does not establish supported capacity, durability or upgrades.

Svelte check/build, 70 library, 46 listing, 29 upload-contract and 14 real TLS journey tests passed, as did release all-target Clippy and seven harness-helper groups. Browser checks cover bounded paging, pending controls, retry/stale errors, focus/session/navigation races, restore/reset/uncertainty and target inspection beyond page one. The focused 101-test suite and final 30-test Trash/smoke sweep passed; after review fixes, all 30 Trash tests passed, including Unicode ordering, malformed rows and stale-restart failure recovery. Quick Trash mode also passed with 98 deleted entries and no Next control (`2026-11-15T10-30-15.854Z-quick`); workload metadata now records one observation per page/action. Desktop/narrow screenshots in both themes were inspected; Impeccable's mechanical detector returned no findings. Required Linux CI and beta qualification remain separate gates.

## Proposed budgets

These are candidate goals for a later optimization, not acceptance gates or claims that the current application meets them:

- On this declared local workload, keep routine first-page browsing below 100 ms median API time and browser action completion below 250 ms.
- Target search completion below 500 ms, including the current 250 ms typing debounce, at 100,000 entries.
- Keep browsing and selective search usable during an upload batch; compare their measured latency with their same-scale baseline before setting an interference budget.
- Reduce demonstrated repeated metadata work in the 60-file UI journey before changing upload concurrency or storage safety boundaries.

The intended self-hosted machine, physical storage, network, concurrent accounts, power-loss durability and upgrade behavior remain unqualified. The million-entry extension is optional and was not run; it would extend beyond the required scales without resolving intended-host qualification.

## Verification

- `npm --prefix web run check`: no errors or warnings; compiled assets rebuilt before Cargo.
- `cargo fmt --check` and release `cargo clippy --locked --release --all-targets --features test-harness -- -D warnings`: passed.
- Release fixture compiled; a normal Cargo invocation printed its opt-in skip and exited successfully.
- Quick full workflow and narrow transfer pass passed. Full-scale partial/failure evidence is listed above, with raw samples and exact source snapshots in ignored reports.
- Failure-message redaction was checked with synthetic password, cookie, CSRF and local-path values. TLS trust remained restricted to fixture credentials.
- No Linux/container/TLS deployment qualification, power-loss test or full application correctness rerun was claimed. Production behavior and UI source were unchanged.
