# Beta releases

The public distribution is a GitHub prerelease containing a compressed Docker
image archive, SHA-256 checksums, image ID, source metadata, Compose configuration
and the standalone [installation guide](docker-beta.md). No registry account is
required. Docker/Linux amd64 is primary; native LXC is secondary and needs its own
qualification. No automated deployment is configured.

## Checks

The CI workflow runs `scripts/check.sh` and `scripts/check-beta.sh` on Ubuntu
24.04 x86_64. The Linux recovery workflow exercises a disposable ext4 bind mount,
two accounts, exact bytes, access denial, restart and stopped-service restore.
Ignored ownership fault cases remain unqualified, not passes. These checks do
not prove physical power-loss durability or schema upgrade/downgrade support.

`check.sh` requires the pinned Rust toolchain, Node/npm, Python 3.11+, native build
tools and cargo-deny 0.20.2. `check-beta.sh` also requires Chromium and NSS
`certutil`. Recovery requires Docker Buildx, Node, sudo and OpenSSH server on a
disposable Linux host. The workflows install their own prerequisites.

## Prepare and publish

1. Commit the source. Choose an unused version of the form `vX.Y.Z-beta.N`.
2. Run the **Beta release** workflow on `main`, supplying that version. Leave
   `publish` false to qualify without publishing. Normal pushes never publish.
3. The workflow runs source/browser checks, builds the Linux amd64 image, and
   runs recovery against that exact image ID. A failed check prevents publication.
4. With `publish` true, the workflow publishes only from the public canonical
   repository's `main` branch. It creates a new prerelease at the checked commit.
   It checks asset names while the release is a draft, then verifies anonymous
   downloads and checksums after publishing. A failed download check returns the
   release to draft. An existing release/tag must not be reused; use a new beta number for changes.
5. Confirm all assets download without authentication, check `SHA256SUMS`, and
   follow the installation guide on the target host. Record that host's observed
   limitations before inviting testers. Feedback goes to hello@corvus-dev.com.

For a local packaging check, run
`scripts/package-beta.sh vX.Y.Z-beta.N /path/to/new-output-directory` from a clean
checkout. This builds but neither publishes nor qualifies the result. Generated
metadata records the source commit and image ID; checksums detect changed assets.
The workflow run provides build provenance. No cryptographic signature or
reproducible-build claim is made.

Keep the prior image and a verified stopped-service backup before upgrading.
Never downgrade a root that the new version may have changed. Restore the matching
pre-upgrade backup into a fresh directory, as described in the installation guide.
