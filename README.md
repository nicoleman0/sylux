# Sylux

Self-hosted multi-user file storage with a Rust server, Svelte interface and SQLite metadata.

## Beta

Docker is the primary installation option. Start with the [Docker beta guide](docs/docker-beta.md).
It covers installing a versioned image, local accounts, private HTTPS access, backup and restore.
Native LXC installation is secondary and has separate qualification limits.

The beta includes local account administration, isolated private storage, folders,
file moves, recoverable deletion and replacement uploads. OIDC, shared folders,
public sharing links, previews and WebDAV are not included.

Keep independent copies of important files. Functional tests do not establish
physical power-loss durability or schema upgrade/downgrade support.

## Feedback

Email **hello@corvus-dev.com** with the version, host environment, steps to reproduce,
expected result and observed result. Do not send passwords, setup tokens or personal files.

## Build and verify

The toolchains and dependencies are pinned. The reference environment is Ubuntu
24.04 x86_64 with Docker, local ext4 storage and one Sylux instance.

- `scripts/check.sh`: web/Rust builds, lint, tests and dependency checks.
- `scripts/check-beta.sh`: feature-enabled contract, integration and browser checks.
- `scripts/syl7-linux.sh`: disposable Docker install, isolation, restart and restore checks.

See [release preparation](docs/releasing.md) and [dependency notices](docs/dependency-notices.md)
for prerequisites, qualification limits and distribution details.

Licensed under the [MIT License](LICENSE).
