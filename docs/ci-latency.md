# CI latency

## Baseline and scope

[Successful qualification 43481498292](https://github.com/nicoleman0/sylux/actions/runs/43481498292) took 655 seconds end to end, including a 3-second queue and a 652-second qualification job. Tool setup took 107 seconds, combined source/browser checks 399 seconds, packaging 115 seconds, image recovery 13 seconds and artifact upload 4 seconds. These are workflow step timings, not isolated compilation measurements.

[Publication run 43483618291](https://github.com/nicoleman0/sylux/actions/runs/43483618291) provides a newer baseline: qualification took 692 seconds after a 4-second initial queue; tools took 110 seconds, source/browser checks 431 seconds, packaging 119 seconds and recovery 11 seconds. Its 745-second total includes publication (25 seconds), GHCR (19 seconds) and job gaps. Compare qualification jobs when measuring cache effects.

Use `python3 scripts/release-timings.py RUN_ID...` to report completed first attempts. Record runner image versions and restored cache keys from the run logs alongside the timings; the script does not classify cache hits.

The release workflow now separates cargo-deny installation, source checks, browser prerequisite installation and beta qualification. Every check still runs on a cache hit. Packaging the checked revision, image recovery, artifact integrity checks and publication guards remain unchanged. Pull request CI is unchanged.

## Cache boundaries

- Pull request CI caches its pinned cargo-deny executable. Its exact key includes runner OS, architecture, image version, the CI workflow and Rust toolchain file. It has no fallback keys.
- Release cargo-deny has a separate `release-cargo-deny` key with runner OS, architecture, image version and a hash of `beta-release.yml` plus `rust-toolchain.toml`. It caches only `$RUNNER_TEMP/cargo-deny`, has no fallback keys and installs cargo-deny 0.20.2 with locked dependencies on a miss.
- Release host Rust dependencies use pinned [Swatinem/rust-cache](https://github.com/Swatinem/rust-cache/tree/d1527c8f67ef9af357290475d4af4b0fab8f12e0). The `release-host-v1` prefix includes the runner image version and release workflow hash. The action adds its job, compiler, environment, manifest, lockfile and toolchain keys. It may reuse dependencies from an earlier lockfile; Cargo still checks changed inputs. Cargo binaries and workspace crates are excluded. Incremental artifacts are removed by the action. Change the prefix to discard this cache generation.

Both release caches separate entries when the release workflow or runner image changes. A compiler change separates Rust artifacts. Changes to other toolchain settings or manifests change its exact key, but may restore earlier dependencies through its fallback key; Cargo checks the current inputs. Cargo-deny requires an exact toolchain-file match. No cache paths include runtime state, browser profiles, private test evidence or credentials. Host dependency artifacts do not cache the Docker build or the Sylux executable.

[GitHub cache scope](https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows#restrictions-for-accessing-a-cache) limits restoration by branch and pull request merge ref. Pull requests can restore default-branch caches; their own caches do not flow back to the default branch. Cache contents are trusted executable build inputs from the allowed scope, never evidence that checks passed. Delete suspect entries or change the key before reuse.

## Deferred caches

Browser download took about 9 seconds in the baseline logs. Keep fresh browser installation and isolated trust/profile setup; a browser cache adds little value at that cost. Docker packaging took 115 seconds, but persistent Buildx caching would change the packaging boundary. Assess that separately, preserving the archive of HEAD, revision labels, image ID and recovery checks.

## Verification procedure

Dispatch `beta-release.yml` on the feature branch with a valid unused beta version and `publish=false`. Record run IDs, revisions, runner images, cache keys and complete gate results.

| Input | Expected observation |
| --- | --- |
| First run with absent release caches | Both caches miss; cargo-deny installs; all qualification and recovery checks pass. |
| Same workflow/toolchain and runner image | Both caches hit; cargo-deny installation is skipped; every check still runs. |
| Changed web input | Dependencies may hit, but Sylux rebuilds. The TLS asset test compares served index bytes with the freshly built web/dist hash. |
| Changed lockfile or toolchain settings | Exact Rust key changes; fallback dependencies remain eligible and Cargo revalidates them. Cargo-deny misses on toolchain-file changes. |
| Changed compiler, runner image or release workflow | Prior Rust artifacts are outside the new compiler/prefix key. Runner/workflow changes also separate cargo-deny entries. |

For a cold repeat, list caches with `gh cache list --ref refs/heads/BRANCH` and delete only the IDs for that branch's release prefixes using `gh cache delete ID`. Do not clear unrelated CI or npm caches. The cold measurement here concerns the two added caches, not every runner dependency.

[Cold qualification 43486047136](https://github.com/nicoleman0/sylux/actions/runs/43486047136) passed on runner image `20261111.370.1`. Both added caches missed. End to end was 678 seconds, including a 3-second queue and 675-second job. Cargo-deny installation took 99 seconds; source checks 56 seconds; browser setup 15 seconds; beta checks 321 seconds; packaging 115 seconds; recovery 12 seconds. Saving the two caches took 6 seconds. The existing npm cache hit, so this is cold for the added release caches only.

[Warm qualification 43487917552](https://github.com/nicoleman0/sylux/actions/runs/43487917552) passed on the same runner image with exact hits for both caches. End to end fell from 11m18s to 7m44s, a 3m34s reduction (32%) in this pair of runs. This measures qualification with publication disabled; publication and GHCR time are excluded.

| Step | Cold seconds | Warm seconds |
| --- | ---: | ---: |
| Initial queue | 3 | 4 |
| Set up job / checkout | 6 | 3 |
| Rust toolchain | 9 | 8 |
| Restore Rust dependencies | 1 | 2 |
| Node setup / npm cache restore | 11 | 2 |
| Install npm and OS check tools | 16 | 12 |
| Restore cargo-deny | 0 | 0 |
| Install cargo-deny | 99 | skipped |
| Source checks | 56 | 26 |
| Browser prerequisites | 15 | 15 |
| Beta qualification | 321 | 265 |
| Packaging | 115 | 113 |
| Packaged-image recovery | 12 | 10 |
| Artifact upload | 3 | 1 |
| Save release caches | 6 | 1 |
| Qualification job total | 675 | 460 |
| End to end | 678 | 464 |

The warm revision added a temporary metadata marker to `web/index.html`. The logs show Sylux recompiling, and the real TLS asset test passed against the current web/dist hash. Both runs passed the full Rust, browser, smoke and packaged-image recovery gates. The marker was removed after verification. Lockfile/compiler changes were not exercised in these runs; their expected behavior above follows the pinned cache action's key construction. This is one cold/warm sample, not a timing guarantee.

Both runs used these keys (miss then exact hit):

```text
release-cargo-deny-Linux-X64-20261111.370.1-667628d943d8159ce0ad1043c21a3f090b4e3c78f0a4f00784d7ad36dff963cf
release-host-v1-20261111.370.1-5def8629f04eb4ba67771a821d4ef1077801124d677bace3ec9e4586ac2b7214-qualify-Linux-x64-01b81e85-cac136ed
```

The added caches occupied about 232 MiB. Browser setup, beta tests and fresh Docker packaging remain on the critical path. Buildx layer caching and parallelizing independent checks are possible follow-ups; neither was implemented here.
