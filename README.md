<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
    <img src="docs/assets/logo-light.svg" alt="Sylux" width="96" height="96">
  </picture>
</p>

<h1 align="center">Sylux</h1>

<p align="center">
  <a href="https://github.com/nicoleman0/sylux/actions/workflows/ci.yml"><img src="https://github.com/nicoleman0/sylux/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status"></a>
  <a href="https://github.com/nicoleman0/sylux/releases"><img src="https://img.shields.io/github/v/release/nicoleman0/sylux?include_prereleases&label=release" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/nicoleman0/sylux" alt="MIT license"></a>
</p>

Self-hosted multi-user file storage with a Rust server, Svelte interface and SQLite metadata.

## Who it is for

Self-hosters and small groups who want private file storage in a browser. Each account's files are private to it. Sylux favors everyday workflows and low operating overhead over broad feature parity.

## Features

- Local accounts with administrator and user roles.
- Folders, file moves, rename and recoverable deletion with [Trash and restore](docs/trash.md).
- Replacement uploads, and [uploads up to 64 GiB that resume](docs/uploads.md) after a connection loss or tab reload, within a 24-hour window.
- [Folder and selection downloads](docs/archives.md) as one ZIP.
- [Folder listings](docs/folder-listings.md) sorted by name, file size or modified date.
- [Account search](docs/search.md) by name.
- Text, image and [PDF previews](docs/pdf-preview.md) in the browser.
- Administrator-set [instance name and appearance](docs/instance-appearance.md).

Not included: OIDC, shared folders, public sharing links and WebDAV.

## Install

Docker is the primary option. Follow the [Docker beta guide](docs/docker-beta.md). It covers installing a versioned image, local accounts, private HTTPS access, backup and restore. [Native LXC installation](docs/lxc.md) is secondary and has separate qualification limits. Lost administrator credentials can be [recovered offline](docs/docker-beta.md#recover-administrator-access) with host access and the service stopped. The release image is 37 MB compressed, uses about 49 MiB of memory idle and 60 MiB under an upload workload, and is ready in under 0.1 seconds on a small x86_64 host; see [the measurements](docs/footprint.md) for the workload and limits.

## Status and limits

Sylux is in beta. Releases are [pre-releases](https://github.com/nicoleman0/sylux/releases). Functional tests do not establish physical power-loss durability or schema upgrade and downgrade support. Keep independent copies of important files.

## Feedback and contributing

Email **hello@corvus-dev.com** with the version, host environment, steps to reproduce, expected result and observed result. Do not send passwords, setup tokens or personal files. There is no contributing guide yet; open an issue or email before sending a pull request.

## License

[MIT](LICENSE). See [third-party notices](docs/dependency-notices.md).
