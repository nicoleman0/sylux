# Sylux

Self-hosted multi-user file storage with a Rust server, Svelte interface and SQLite metadata.

## Beta

Docker is the primary installation option. Start with the [Docker beta guide](docs/docker-beta.md).
It covers installing a versioned image, local accounts, private HTTPS access, backup and restore.
[Native LXC installation](docs/lxc.md) is secondary and has separate qualification limits.

The beta includes local account administration, isolated private storage, folders,
file moves, recoverable deletion and replacement uploads. OIDC, shared folders,
public sharing links and WebDAV are not included. Text, image and
[PDF previews](docs/pdf-preview.md) are available in the browser.

[Folder listings](docs/folder-listings.md) support name and file-size sorting.

Keep independent copies of important files. Functional tests do not establish
physical power-loss durability or schema upgrade/downgrade support.

## Feedback

Email **hello@corvus-dev.com** with the version, host environment, steps to reproduce,
expected result and observed result. Do not send passwords, setup tokens or personal files.

Licensed under the [MIT License](LICENSE). See [third-party notices](docs/dependency-notices.md).
