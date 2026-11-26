# Rehearse a populated Docker upgrade and rollback

Run **Packaged Docker upgrade and rollback rehearsal** in GitHub Actions. A pull request
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

## Run the recovery in the same job

The workflow runs these phases in order on the same runner. Cleanup runs
even if an earlier step fails.

```sh
scripts/rehearse-docker-upgrade.sh upgrade
scripts/rehearse-docker-upgrade.sh rollback
scripts/rehearse-docker-upgrade.sh cleanup
```

1. Upgrade seeds the prior release, captures the expected inventory and
   access results, and makes a stopped-service archive on the separate
   ext4 mount. It verifies beta.10 before recovery begins.
2. Rollback checks that access is restricted to host loopback, stops beta.10,
   and confirms a clean exit and a closed listener. It rechecks the archive
   against the SHA-256 recorded before the upgrade. It also verifies the
   retained beta.9 bundle against its pinned release manifest and checks the
   same immutable local image ID.
3. Rollback creates a separate empty root on local ext4 and restores the
   archive there with numeric ownership and permissions. It starts beta.9
   with its shipped Compose and a copy of its configuration pointing to the
   restored root. The beta.10 root is kept through verification.
4. The driver compares two fresh sign-ins and account roles, private inventory,
   exact downloads, Trash restore bytes, cross-account denials, and anonymous
   access to the working and revoked links against the beta.9 baseline.
   Named library grants created on beta.10 exercise the upgrade phase.
   The restored listener stays on loopback through teardown.

The GitHub Actions summary records the release revision, package and stopped
archive checksums, loaded image ID, mount checks and comparison results.
The private workspace is `$RUNNER_TEMP/sylux-upgrade`. Cleanup removes the
rehearsal containers, unmounts the archive and removes the workspace with
both roots, the archive, TLS key, baselines and private log. Only redacted
evidence leaves the runner.

For a real recovery, follow the [Docker operator guide](docker-beta.md#5-upgrade-and-roll-back),
including owner review of access before reopening it.

## Reuse for the stable candidate

Update the release pins to beta.10 and the published stable bundle. Seed
beta.10 and run the driver's `grants` phase there before the stopped archive.
Keep the resulting `expected-grants.json` alongside `expected.json`. After
the stable upgrade, run `verify`, `trash-bytes` and `verify-grants` against the
same fixture directory. The last phase verifies the revoked library remains
unavailable and the active library remains the member's only incoming share,
with its exact listing and download bytes intact. For the rollback rehearsal,
create the named-grant baseline on beta.10 before archiving and verify it again
after restoring beta.10 with `verify-grants`. The current beta.9 rehearsal does
not change; it has no library or sharing routes.
