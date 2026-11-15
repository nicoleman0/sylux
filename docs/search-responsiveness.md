# Live search scheduling

SYL-63 changes the live-search delay from 250 ms to 120 ms. Enter submits
immediately. Clear restores browsing immediately. Composition waits for the
committed IME value. Request cancellation and generation checks remain in place.
Search still uses the account-wide API, including its existing pagination.

## Comparison before implementation

Inspected 15 November 2026:

- [Nextcloud 35 searching and filtering](https://docs.nextcloud.com/server/stable/user_manual/en/files/access_webgui.html#searching-and-filtering), including its
  [search screenshot](https://docs.nextcloud.com/server/stable/user_manual/_images/files_page-7.png).
  The screenshot shows a text field with an explicit clear control, a query chip
  and file results. Adopt a reachable clear control and named account-wide search.
  Keep Sylux's existing field and results layout. Reject adding another
  current-view filter or query-chip layer for this change.
- [File Browser Search.vue at 833d908](https://github.com/filebrowser/filebrowser/blob/833d908884d5c801f30f5c098d7977177eb3a36b/frontend/src/components/Search.vue).
  The template provides Enter submission, an active-search indicator and a
  stop/close action. Its submit handler aborts the previous request. Adopt Enter
  and cancellation. Keep automatic live search instead of requiring Enter.
  No search screenshot or live session was available for File Browser; this part
  is a source comparison, not a visual or live-behavior comparison.

No live competitor timing was measured. The Nextcloud screenshot cannot establish
its debounce duration. Neither source sets Sylux's delay; the measurements below
do that.

## Scheduling experiment

Baseline source: `82f1455`. Ran on 15 November 2026 with Node 24.22.2,
Playwright 1.65.0 and its Chromium headless shell build 1254 on macOS ARM64.
No other browser suite ran during timing measurements.

The opt-in measurement in `web/tests/search-responsiveness.spec.ts` uses the real
input, fetch and DOM update path. Each sample enters five characters at a fixed
60 or 100 ms gap. The mocked API returns one matching file after a 40 or 300 ms
wait. Each combination runs three times. Only the timer value changed between
250, 0, 75 and 120 ms runs; the fixture and machine remained the same.

Measured intervals use browser `performance.now()`: final input event to fetch
start, fetch start to response, and response to the correct visible result's DOM
update. Request counts are observed at the intercepted API route. The DOM
observer also checks for any visible search row that does not match the current
input. This measures DOM availability, not display paint or human response time.

Medians in milliseconds, with requests per five-character burst:

| Delay | Key gap | API wait | Schedule | Fetch | DOM | Input to results | Requests |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 250 | 60 | 40 | 251.9 | 46.9 | 5.6 | 304.4 | 1 |
| 250 | 60 | 300 | 251.4 | 306.2 | 5.7 | 563.3 | 1 |
| 250 | 100 | 40 | 251.4 | 46.5 | 5.7 | 303.6 | 1 |
| 250 | 100 | 300 | 251.2 | 306.3 | 5.0 | 562.7 | 1 |
| 0 | 60 | 40 | 1.8 | 44.6 | 5.1 | 51.5 | 5 |
| 0 | 60 | 300 | 1.7 | 305.0 | 5.8 | 312.5 | 5 |
| 0 | 100 | 40 | 3.2 | 45.5 | 5.2 | 54.0 | 5 |
| 0 | 100 | 300 | 1.3 | 304.7 | 3.8 | 310.4 | 5 |
| 75 | 60 | 40 | 76.8 | 46.9 | 5.9 | 129.6 | 1 |
| 75 | 60 | 300 | 77.1 | 305.6 | 4.8 | 388.0 | 1 |
| 75 | 100 | 40 | 77.3 | 46.0 | 5.6 | 128.9 | 5 |
| 75 | 100 | 300 | 77.0 | 305.4 | 5.1 | 388.6 | 5 |
| 120 | 60 | 40 | 121.5 | 46.7 | 6.0 | 173.7 | 1 |
| 120 | 60 | 300 | 121.9 | 305.4 | 5.4 | 432.8 | 1 |
| 120 | 100 | 40 | 121.4 | 47.1 | 5.2 | 173.7 | 1 |
| 120 | 100 | 300 | 121.8 | 306.9 | 5.3 | 435.0 | 1 |

All 48 samples recorded zero stale renders. Median columns are calculated
independently, so their rounded values need not add to the median total.
[Raw samples](search-scheduling-2026-11-15.json) retain each interval.

120 ms is the lowest tested delay that coalesced both typing rates into one
request. It reduced final-input-to-results by about 128–131 ms from the baseline.
75 ms sent five requests for the 100 ms gap, including with slow responses;
abort does not prove the server avoided work. Zero delay sent five in every
case. These small mocked samples establish scheduling behavior, not server
throughput, backend search latency, large-account performance or deployment
qualification. Slower typing and pauses longer than 120 ms can still send
intermediate queries.

Reproduce a candidate by changing only the timer in `App.svelte`, then running:

```sh
SEARCH_MEASURE=120 npm --prefix web run smoke -- search-responsiveness --grep 'measure search' --reporter=list
```

The environment value labels the output; it does not change the application.
Use the pinned tool versions and restore the selected 120 ms timer afterward.

## Regression coverage

The normal browser suite covers the 120 ms boundary, request coalescing,
immediate Enter with duplicate prevention, both clear paths, IME composition
including a transient empty candidate, navigation with scheduled and in-flight
work, late responses when transport cancellation is deliberately ignored,
failed-search retry and search during an active upload with completion
revalidation. Viewport cases capture pending and successful search at 390 and
1280 px in both themes and check focus, busy semantics and page overflow.

These fixtures mock the API. Native IME input on macOS, other browser engines,
real TLS, deployed latency and backend upload/storage correctness are outside
this evidence.
