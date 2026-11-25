# Rehearse a populated Docker upgrade

Run **Packaged Docker upgrade rehearsal** in GitHub Actions. A pull request
that changes the rehearsal triggers it automatically. Once the workflow
exists on the default branch, you can also dispatch it on a branch containing
the workflow. It uses a disposable hosted Ubuntu 24.04 x86_64 runner and
requires public access to both pinned GitHub release bundles. It uses no
repository secrets or write permissions.

The job follows the [manual Docker upgrade](docker-beta.md#5-upgrade-and-roll-back)
from beta.9 to beta.10 with each release's shipped Compose file. The live root
is on local ext4. The stopped backup is on a separately mounted loop-backed
ext4 filesystem on the same VM. The driver uses the app's browser-facing HTTP
flows over certificate-verified host loopback TLS. The job summary records
the release revisions, checksums, image IDs, mount evidence and results.

## Reuse in the rollback job

Run `scripts/rehearse-docker-upgrade.sh upgrade` first. Add recovery steps
after it and before `scripts/rehearse-docker-upgrade.sh cleanup` in the
**same job**. Keep the endpoint restricted to host loopback. The private
workspace is `$RUNNER_TEMP/sylux-upgrade`:

- `v0.1.0-beta.9/` holds the verified release package, unchanged prior
  `.env`, shipped Compose file and `local-image-id`. The prior image is
  loaded. Use Compose project name `sylux-upgrade-rehearsal` for both releases.
- `archive/backup.tar` and `archive/backup.tar.sha256` are root-private
  on the archive mount. Check the archive before restoring with
  `sudo sh -c 'cd "$1" && sha256sum -c backup.tar.sha256' sh "$RUNNER_TEMP/sylux-upgrade/archive"`.
  Stop beta.10, restore into a fresh root and
  change the prior configuration's data directory as described in the
  [rollback procedure](docker-beta.md#5-upgrade-and-roll-back).
- `trust.pem` and `expected.json` support a fresh comparison with
  `python3 scripts/rehearse-docker-upgrade.py verify "$RUNNER_TEMP/sylux-upgrade"`
  after readiness on the restored beta.9 listener. The JSON contains fixture
  credentials and private link data. Keep it on the runner with the archive,
  TLS key and private logs through cleanup.

## Reuse for the stable candidate

Update the release pins to beta.10 and the published stable bundle. Seed
beta.10 and run the driver's `grants` phase there before the stopped archive.
Keep the resulting `expected-grants.json` alongside `expected.json`. After
the stable upgrade, run `verify`, `trash-bytes` and `verify-grants` against the
same fixture directory. The last phase compares the named library and owner
download and checks that the member still cannot list or download from that
library.
