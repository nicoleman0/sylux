# CI latency

## Baseline and scope

[Successful qualification 43481498292](https://github.com/nicoleman0/sylux/actions/runs/43481498292) took 655 seconds end to end, including a 3-second queue and a 652-second qualification job. Tool setup took 107 seconds, combined source/browser checks 399 seconds, packaging 115 seconds, image recovery 13 seconds and artifact upload 4 seconds. These are workflow step timings, not isolated compilation measurements.

[Publication run 43483618291](https://github.com/nicoleman0/sylux/actions/runs/43483618291) provides a newer baseline: qualification took 692 seconds after a 4-second initial queue; tools took 110 seconds, source/browser checks 431 seconds, packaging 119 seconds and recovery 11 seconds. Its 745-second total includes publication (25 seconds), GHCR (19 seconds) and job gaps. Compare qualification jobs when measuring cache effects.

Use `python3 scripts/release-timings.py RUN_ID...` to report completed first attempts.

The release workflow now separates cargo-deny installation, source checks, browser prerequisite installation and beta qualification. Every check still runs on a cache hit. Packaging the checked revision, image recovery, artifact integrity checks and publication guards remain unchanged. Pull request CI is unchanged.

## Cache boundaries

- Pull request CI caches its pinned cargo-deny executable. Its exact key includes runner OS, architecture, image version, the CI workflow and Rust toolchain file. It has no fallback keys.
- Release cargo-deny has a separate `release-cargo-deny` key with runner OS, architecture, image version and a hash of `beta-release.yml` plus `rust-toolchain.toml`. It caches only `$RUNNER_TEMP/cargo-deny`, has no fallback keys and installs cargo-deny 0.20.2 with locked dependencies on a miss.
- Release host Rust dependencies use pinned [Swatinem/rust-cache](https://github.com/Swatinem/rust-cache/tree/d1527c8f67ef9af357290475d4af4b0fab8f12e0). The `release-host-v1` prefix includes the runner image version and release workflow hash. The action adds its job, compiler, environment, manifest, lockfile and toolchain keys. It may reuse dependencies from an earlier lockfile; Cargo still checks changed inputs. Cargo binaries and workspace crates are excluded. Incremental artifacts are removed by the action. Change the prefix to discard this cache generation.

Both release caches invalidate when the release workflow changes. A runner image or toolchain change also separates them from prior entries. No cache paths include runtime state, browser profiles, private test evidence or credentials. Host dependency artifacts do not cache the Docker build or the Sylux executable.

[GitHub cache scope](https://docs.github.com/en/actions/using-workflows/caching-dependencies-to-speed-up-workflows#restrictions-for-accessing-a-cache) limits restoration by branch and pull request merge ref. Pull requests can restore default-branch caches; their own caches do not flow back to the default branch. Cache contents are trusted executable build inputs from the allowed scope, never evidence that checks passed. Delete suspect entries or change the key before reuse.

## Deferred caches

Browser download took about 9 seconds in the baseline logs. Keep fresh browser installation and isolated trust/profile setup; a browser cache adds little value at that cost. Docker packaging took 115 seconds, but persistent Buildx caching would change the packaging boundary. Assess that separately, preserving the archive of HEAD, revision labels, image ID and recovery checks.

Cold/warm release measurements and changed-input invalidation results remain to be recorded. Qualification runs for this work use `publish=false`; no performance claim follows from configuration alone.
