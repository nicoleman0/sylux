# CI latency

The pull request CI workflow caches the compiled cargo-deny tool. A miss installs the pinned version from source with its locked dependencies. The key includes the runner OS, architecture and image version, the CI workflow and the Rust toolchain file. Changes to the tool installation or toolchain invalidate the cache. There are no fallback keys.

GitHub scopes caches by branch and pull request merge ref. A pull request can restore its own cache or a default-branch cache; its cache is not available to the default branch. The cached executable is trusted as code produced by that scope. The release workflow builds its own cargo-deny and does not restore this cache. Delete the cache or change the key if its contents are in doubt. The cache contains no credentials or test data.

The source, browser, container recovery and release checks still run. A cache hit avoids only the cargo-deny installation.
