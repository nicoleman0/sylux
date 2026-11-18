# Delivery latency

## Pickup to merge, 18 November 2026

Times are UTC. Pickup means the issue entered In Progress, not when it
entered Todo. [SYL-95](https://linear.app/corvus-dev/issue/SYL-95) took
19m24s from pickup to merge; [SYL-77](https://linear.app/corvus-dev/issue/SYL-77)
took 129m57s. These changes ran together, so their durations must not be
added to describe batch elapsed time. The batch ended at 10:46:13,
129m57s after the earlier pickup.

| PR | Milestone | UTC | Evidence |
| --- | --- | --- | --- |
| [71](https://github.com/nicoleman0/sylux/pull/71) | Pickup | 08:36:23.783 | Linear state history |
| 71 | Initial commit, then SVG correction | 08:38:46 / 08:39:37 | PR commits |
| 71 | Builder handed off final head | 08:40:38 | Worker receipt |
| 71 | Independent screen finished, four doubts | 08:43:21 | Worker receipt |
| 71 | Full review started | 08:43:55 | Task creation |
| 71 | Final-head CI complete | 08:48:28 | [CI](https://github.com/nicoleman0/sylux/actions/runs/44268918777) |
| 71 | Full review finished, final-head renders verified | 08:54:57 | Worker receipt |
| 71 | Merged | 08:55:48 | PR merge event |
| [72](https://github.com/nicoleman0/sylux/pull/72) | Pickup | 08:36:15.603 | Linear state history |
| 72 | Accounting question answered | 08:39:57 | Worker reply; Linear comment at 08:40:02 |
| 72 | Initial commit / PR opened | 09:12:47 / 09:14:00 | PR commits and creation |
| 72 | Builder handed off | 09:14:55 | Worker receipt |
| 72 | Independent review started / finished | 09:15:37 / 09:52:06 | Task creation / worker receipt |
| 72 | First retry fix handed off | 09:53:54 | Worker receipt, commit `09b6462` |
| 72 | Targeted review started | 09:54:27 | Task creation |
| 72 | Browser shard rejected the fix / aggregate failed | 09:56:27 / 10:01:26 | [Failed CI](https://github.com/nicoleman0/sylux/actions/runs/44280218511) |
| 72 | Targeted reviewer stopped with report written | 10:25:26 | Worker escalation; report requests changes |
| 72 | Second fix task created | 10:26:30 | Task creation |
| 72 | Second fix handed off | 10:30:26 | Worker receipt, commit `09036b6` |
| 72 | Final independent review finished | 10:34:45 | Worker receipt |
| 72 | Chromium install timed out; aggregate failed | 10:41:04 / 10:41:13 | [CI attempt 1](https://github.com/nicoleman0/sylux/actions/runs/44286031815/attempts/1) |
| 72 | Failed jobs rerun / CI green | 10:41:42 / 10:45:22 | [CI attempt 2](https://github.com/nicoleman0/sylux/actions/runs/44286031815/attempts/2) |
| 72 | Merged | 10:46:13 | PR merge event |

Worker receipts and task creation times were read from the completed Orca
runs, alongside the build/review reports. Task creation bounds a phase;
it does not establish when model execution began. GitHub job timestamps
and Linear state history provide the external timing boundaries.

## What determined elapsed time

- PR 71's full review took 11m02s. CI overlapped its first 4m33s;
  only 6m29s of review remained after CI. Merge followed review by 51s.
  Broad CI was selected because new `docs/assets/*.svg` files are unknown
  inputs to the conservative selector. Making SVGs lightweight would save
  runner work here, but would not remove the final rendering-review tail.
- PR 72's first independent review took 36m29s after task creation.
  It checked live accounting, authorization, performance evidence and UI.
  Those checks remain required; the evidence does not establish how much
  of that phase was avoidable. Its initial CI attempt was cancelled at
  09:25:10, during this review.
- The largest evidenced rework interval is 42m39s, from the first review's
  retry finding at 09:52:06 to final approval at 10:34:45. The first fix
  kept the button mounted but native disabling still blurred it. Its test
  raced that asynchronous blur. The fix also moved focus after stale
  requests and retained busy state across sign-out. The targeted review
  spent 30m59s before stopping with its report written; a reporting/quota
  failure then required coordinator recovery. The browser shard had
  already rejected that fix at 09:56:27; the required aggregate failed at
  10:01:26. The second fix task was created at 10:26:30, leaving 30m03s
  after shard failure and 25m04s after the red aggregate before rework
  began. Acting on red CI could have started rework sooner without the
  new command. This coordinator-side wait overlaps the targeted review
  inside the 42m39s window; it is not an additional serial delay. The
  evidence does not establish how much earlier rework would have moved
  final approval or merge.
- Final CI and final review also overlapped. CI's remaining tail after
  approval was 10m37s, followed by 51s to merge. Shard 3 spent 9m53s
  installing Chromium before cancellation; its rerun took 3m29s. Shard 4
  independently ran until 10:38:42, so eliminating the failed install would
  not save the entire install-plus-rerun duration. Against that unchanged
  parallel job, the observed green-gate tail was 6m40s. No download/cache
  improvement is claimed by this change.

The 42m39s rework window is an opportunity bound, not 42m39s of proven
savings. Fixing the defect, independent review, CI and handoffs still cost
time. The completed [CI investigation](ci-latency.md) remains relevant;
adding all job durations would overstate the critical path.

## Earlier feedback for retry changes

Before review of storage retry or shared refresh controls, run:

```sh
npm --prefix web ci  # fresh checkout or changed dependencies/toolchain
npm --prefix web run check:feedback
```

Use the pinned Node/npm versions and an installed Playwright Chromium.
The command runs Svelte checking, then the existing storage-usage and
refresh-feedback suites three times with two workers and no retries.
It stops on the first failure. `CI=true` prevents reuse of an unrelated
dev server; a busy test port fails instead of silently testing stale code.

Coverage includes both storage themes, held-request focus beyond the blur
window, duplicate Enter/Space/click prevention, moved focus, sign-out,
unknown versus zero, and existing refresh consumers. The suites also
capture desktop/mobile screenshots. This is mocked-browser feedback.
It does not replace independent review, the complete browser shards,
Rust/storage checks, authority verification, real TLS or recovery CI.

Include the tested revision, command, result and remaining checks in the
review handoff. A new retry surface still needs its own held-request,
keyboard and session tests; this command only covers the named suites.

## Replay

### Later regression coverage

The same command ran on macOS arm64 with Node 24.22.2, npm 11.19.1,
Playwright 1.65.0 and Chromium. Each historical source used a disposable
copy with its own locked dependency install, identical final tests and
configuration, and a fresh dev server. Dependency installation is outside
these command timings. The accepted source is byte-identical to main at
`1eebbc85e3b9904174f7fc37794098fdd938d60e` for the application and existing
browser inputs. One missing folder-response mock was corrected in the
storage sign-out test and used identically in every final replay.

| Application source | Command elapsed | Result |
| --- | ---: | --- |
| Initial `c646fe30ddedb1f2caee26040b6ad8428b58fc5a` | 12.341s | Fails: busy retry trigger disappears |
| First fix `09b64624478abd0bae88ba93506f94f0bd632c05` | 12.336s | Fails: busy trigger is native-disabled rather than aria-disabled |
| Accepted `09036b634d2b37235d1cfb2626e8f68ba79ceef5` | 31.995s | Svelte check clean; 87 browser executions pass |

These replays use later regression tests, not the tests present at each
historical commit. They show that current coverage rejects the known bad
implementations; their short failure times do not establish earlier
feedback that the procedure alone would have provided at the time.

The failing runs stop early: each has one expected failure, 23 passes, one
interrupted concurrent test and 62 unrun executions. They establish early
rejection, not complete historical coverage. The passing run exercises all
29 tests three times, including the held-request and session cases in both
themes. The 400ms focus hold and native-disabled assertion are preserved.
The initial current-source run also passed all 87, but emitted errors from
the incomplete sign-out mock; the final corrected run has no such errors.

### Historical test coverage

Independent review also replayed the historical sources with their own
storage-usage and refresh-feedback tests, using the new repeated command:

| Source and its own tests | Command elapsed | Result |
| --- | ---: | --- |
| Initial `c646fe3` | 20.2s | Passes: 63 executions; held-request regression tests were absent |
| First fix `09b6462` | 22.98s | Fails on first repeat: pending retry loses keyboard focus, light theme |

For `09b6462`, two separate single-pass runs produced one pass (23 tests)
and one failure on the same focus test in the dark theme. Repetition
exposed a race already covered by that commit's own test. The initial
commit's passing replay means this procedure alone would not have caught
its defect before the original review.

The first fix was handed off at 09:53:54; hosted CI first rejected it at
09:56:27, 2m33s later. Its own-test replay failed in 22.98s, about **2m10s
earlier feedback** if run at that handoff boundary, excluding dependency
installation. This is a retrospective feedback comparison, not observed
delivery savings. Rejection before handoff could have avoided the targeted
review cycle bounded by task creation at 09:54:27 and the second fix task
at 10:26:30 (32m03s), within the 42m39s rework window. That interval includes
the overlapping red-gate wait above, so these opportunities must not be
added. Actual fixes, independent review and final CI still take time;
no removable fraction of the cycle or resulting merge-time saving has
been measured.

Actual pickup-to-merge savings remain unmeasured: no comparable new PR has
merged under this procedure. These runs support earlier rejection of the
first fix and current regression protection, not discovery of every new
retry defect. Linux, real TLS and container recovery were not rerun
locally; their required hosted gates are unchanged. No release or
deployment was made.
