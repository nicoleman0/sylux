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

Sylux is a self-hosted file server for small groups. Each account has a private library. Use the browser to organize files, share them, and recover items from Trash. A single Rust service serves the Svelte interface and stores metadata in SQLite.

## Install Sylux

For a fresh installation, use Ubuntu 24.04 x86_64 with systemd, local ext4 storage under `/srv`, and port 3000 free. Run this in a terminal with curl, Python 3, and sudo access.

```sh
curl -fsSL https://raw.githubusercontent.com/nicoleman0/sylux/main/install.sh | sh
```

This command becomes available when GitHub lists `v0.1.0` as the latest stable release. To install [beta.10](https://github.com/nicoleman0/sylux/releases/tag/v0.1.0-beta.10), replace `| sh` with `| sh -s -- --beta`.

The installer shows its proposed host changes before asking for confirmation. Follow the [Docker quick start](docs/docker-beta.md#docker-quick-start) to trust the local certificate, create your administrator account, and upload your first file.

### Already run Docker?

See the [Docker Hub repository](https://hub.docker.com/r/nicoleman/sylux) and the [manual Compose instructions](docs/docker-hub-mirror.md#use-the-mirrored-image-with-compose) for the registry install. Check the registry for published tags before choosing a version.

## Working with files

Check the guide for your release for feature availability.

- [Resume interrupted uploads](docs/uploads.md) of up to 64 GiB. Search by name, preview images, text, and PDFs, or [download a folder as a ZIP](docs/archives.md).
- Share read-only access with another account, make a library available to all signed-in users, or send an [expiring download link](docs/sharing.md) for a single file.
- [Restore files and folders from Trash](docs/trash.md). Connect a desktop client with an app password using [WebDAV](docs/webdav.md).
- Use local accounts or configure [Keycloak sign-in](docs/oidc.md) with administrator approval.

## Operating Sylux

Keep an independent backup of important files. Use the guide shipped with your release for [backup and restore](docs/docker-beta.md#4-stop-back-up-and-restore) and [upgrades](docs/docker-beta.md#5-upgrade-and-roll-back). The [upgrade rehearsal](docs/docker-upgrade-rehearsal.md) records the package versions and environment tested.

[Administrator recovery](docs/docker-beta.md#recover-administrator-access) · [Offline account export](docs/export.md) · [Native LXC installation](docs/lxc.md) · [Resource measurements](docs/footprint.md)

## Feedback and license

Open an issue or email **hello@corvus-dev.com** before sending a pull request. For bug reports, include the version, host, steps to reproduce, and expected and observed results. Omit passwords, setup tokens, and personal files.

[MIT license](LICENSE) · [Third-party notices](docs/dependency-notices.md)
