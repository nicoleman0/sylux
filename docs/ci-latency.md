# CI latency

The pull request CI workflow caches the compiled cargo-deny tool. A miss installs the pinned version from source with its locked dependencies. The key includes the runner OS, architecture and image version, the CI workflow and the Rust toolchain file. Changes to the tool installation or toolchain invalidate the cache. There are no fallback keys.

GitHub scopes caches by branch and pull request merge ref. A pull request can restore its own cache or a default-branch cache; its cache is not available to the default branch. The cached executable is trusted as code produced by that scope. The release workflow builds its own cargo-deny and does not restore this cache. Delete the cache or change the key if its contents are in doubt. The cache contains no credentials or test data.

Pull request CI runs the source checks. The separate container recovery workflow runs for affected paths. Beta contract, integration, real TLS browser and web smoke checks run in the beta release workflow, not on each pull request. A cache hit avoids only the cargo-deny installation.
