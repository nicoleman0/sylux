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

Self-hosters and small groups who want a focused place to store and manage files together. Sylux aims to cover everyday [Nextcloud file workflows](https://docs.nextcloud.com/server/stable/user_manual/en/files/access_webgui.html): accounts, folders, uploads, previews, recovery and file sharing, while keeping the service small to operate. Account administration is occasional, and most file handling happens in a browser.

## Current source capabilities

This inventory describes the current source tree. A merged capability is not necessarily in every published beta; check the selected release's assets and guide before installing. The latest published beta may lag `main`.

- [Local accounts](src/http/accounts.rs) have administrator and user roles.
- Private files and folders, with [Trash and restore](docs/trash.md); move and rename are also supported.
- Replacement uploads, and [uploads up to 64 GiB that resume](docs/uploads.md) after a connection loss or tab reload, within a 24-hour window.
- [Revocable, expiring download links](docs/sharing.md) let an owner share one file with anyone who has the link.
- [Folder and selection downloads](docs/archives.md) as one ZIP.
- [Folder listings](docs/folder-listings.md) sorted by name, file size or modified date.
- [Account search](docs/search.md) by name.
- [Text and image previews](web/src/lib/preview.ts) are available in the browser; [PDF previews](docs/pdf-preview.md) have browser requirements and a download fallback.
- Administrator-set [instance name and appearance](docs/instance-appearance.md).

Optional [WebDAV access](docs/webdav.md) lets desktop clients work with private libraries using revocable app passwords. See the guide for setup and method limits.

Optional [Keycloak sign-in](docs/oidc.md) uses administrator-approved links and keeps a separate local administrator sign-in. Public folder links remain planned. See [sharing](docs/sharing.md) for public single-file links.

## Operating footprint

Release v0.1.0-beta.8 measured 35.2 MiB compressed and 49.2 MiB idle process RSS on a 1 GiB Linux host; startup to ready had an 86 ms median. Those are measurements of one release on one host and workload, not capacity or scaling guarantees. See the [measurement method and limits](docs/footprint.md).

## Install

On Ubuntu 24.04 x86_64 with local ext4 storage, pick a [published beta release](https://github.com/nicoleman0/sylux/releases) that includes `install-sylux.sh`. Download and review the script from that release, then run it with the same tag. For a release with the installer, download it from `https://github.com/nicoleman0/sylux/releases/download/<tag>/install-sylux.sh`. Beta.9 does not include it; use the manual procedure for that release.

```sh
sudo bash ./install-sylux.sh '<published beta tag>'
```

Review the host changes shown by the installer before confirming. When Sylux is ready, follow the [Docker quick start](docs/docker-beta.md#docker-quick-start) to trust the local certificate, create your administrator, and upload a file. For releases without the installer, use the [manual Docker procedure](docs/docker-beta.md#1-get-and-verify-a-published-release).

The Docker guide covers [backup and restore](docs/docker-beta.md#4-stop-back-up-and-restore) and [offline administrator recovery](docs/docker-beta.md#recover-administrator-access). [Native LXC](docs/lxc.md) has a separate procedure and qualification limits. See [the measurements](docs/footprint.md) for release image size, memory and startup time.

Export one account to a plain directory with the server stopped: [offline export](docs/export.md).

## Status and limits

Sylux is in beta. Releases are [pre-releases](https://github.com/nicoleman0/sylux/releases). Functional tests do not establish physical power-loss durability or schema upgrade and downgrade support. Keep independent copies of important files.

## Feedback and contributing

Email **hello@corvus-dev.com** with the version, host environment, steps to reproduce, expected result and observed result. Do not send passwords, setup tokens or personal files. There is no contributing guide yet; open an issue or email before sending a pull request.

## License

[MIT](LICENSE). See [third-party notices](docs/dependency-notices.md).
