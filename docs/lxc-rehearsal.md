# Rehearse Sylux native install, upgrade and restore

Use a fresh disposable CT created for this exercise on the authorized Proxmox
node. Follow [guest provisioning](lxc.md#2-provision-the-guest) and retain its
ownership marker. The expected path is Proxmox VE 9.2 on x86_64, a Debian 13
amd64 template and an ext4 CT root on local LVM-thin. The driver uses one CT
with stock unprivileged isolation. It needs no guest network after prerequisite
installation. Use synthetic data only.

## Prepare the packages

Use a local Linux Docker engine or Docker Desktop's Linux amd64 builder.
Download the published beta.10 **Docker** assets into an empty directory:

```bash
PRIOR=v0.1.0-beta.10
PRIOR_SHA=f69840df3ba30b1f6d98e30a86603afd648c6654
PRIOR_DOCKER_MANIFEST=d6d61886c01d21e21f4bd1a9150423b30d8f88631c53d6e5a2376d133bbff349
PRIOR_DOCKER=$(mktemp -d)
for file in SHA256SUMS compose.yaml docker-beta.md env.example image-id.txt release.json sylux-image.tar.gz install-sylux.sh; do
  curl -q --fail --show-error --location --proto '=https' --proto-redir '=https' \
    --output "$PRIOR_DOCKER/$file" \
    "https://github.com/nicoleman0/sylux/releases/download/$PRIOR/$file" || exit 1
done
SYLUX_NATIVE_HISTORICAL_FIXTURE=1 scripts/package-native.sh \
  "$PRIOR_DOCKER" "$PRIOR" "$PRIOR_SHA" "$PRIOR_DOCKER_MANIFEST" ./prior-native || exit 1
sha256sum ./prior-native/SHA256SUMS-native
```

The packager requires the exact Docker manifest hash and source commit, loads
the archive, and checks the image's platform and OCI labels. It copies the
unchanged executable and notices from that image by its locally loaded ID.
The installer in the resulting private beta.10 fixture comes from the reviewed
candidate checkout. beta.10 had no published native package. Run packaging with
the exact version tag absent from the Docker engine to avoid disturbing an
existing image. The packager leaves the verified image loaded.

For the candidate, use the downloadable `sylux-native-<run-id>-<attempt>`
artifact from `beta-release.yml`. A branch dispatch takes an exact version such
as `v0.1.0` and runs qualification without publishing. Record the checkout SHA,
the workflow attempt, and the native manifest SHA-256 in the job summary.
Extract into an empty directory with just the three native files. To build
locally, commit the source first and run `scripts/package-beta.sh VERSION OUTPUT`.
On an isolated Docker engine with that exact tag absent, run
`scripts/package-native.sh DOCKER_OUTPUT VERSION SOURCE_SHA DOCKER_MANIFEST_SHA NATIVE_OUTPUT`.
Use the `SHA256SUMS` digest from the build output. There is no need for a second
Rust compilation in the native packaging step.

Run the fixture checks locally with:

```bash
scripts/test-native.sh CANDIDATE_BUNDLE CANDIDATE_VERSION CANDIDATE_SOURCE_SHA CANDIDATE_NATIVE_MANIFEST_SHA
```

The test installs Debian dependencies when building its disposable image and
disables networking for the fixture run. It verifies and extracts the package,
runs Sylux under the dedicated account with trusted TLS, and compares uploads
after a clean process restart. It also checks the installer refuses a Docker
guest. This checks the packaged binary and assets. The CT exercise covers the
actual installer and systemd.

## Run on the Proxmox node

Transfer both verified native asset directories and the reviewed source tree
for the candidate to the node. The source can be exported with `git archive`
from the candidate commit. From that tree, as root, run:

```bash
set -o pipefail
scripts/qualify-native-lxc.sh "$CTID" "$MARKER" \
  "$PRIOR_NATIVE" "$PRIOR" "$PRIOR_SHA" "$PRIOR_NATIVE_MANIFEST" \
  "$CANDIDATE_NATIVE" "$CANDIDATE" "$CANDIDATE_SHA" "$CANDIDATE_NATIVE_MANIFEST" \
  2>&1 | tee "$EVIDENCE_LOG"
```

Use separate directories for the two packages and a new private evidence file.
Provide independently recorded native manifest SHA-256 values. Leave the guest
running after provisioning so the driver can check its configuration and stage
files. The driver stops on a configuration, digest, guest identity, filesystem,
readiness, isolation, ownership or byte comparison mismatch. A nonzero exit
requires inspection of the owned CT before retrying. The installation guards
refuse to reinitialize existing data.

The driver performs these checks in order:

1. Install and boot beta.10. Test first setup and sign-in, two private accounts,
   multiple uploaded files, owner isolation and byte-exact TLS downloads.
2. Restart the service, then shut down and start the same CT. Verify the enabled
   service and exact files after each restart. Record the guest root filesystem,
   user namespace map, running service ownership and TLS separation.
3. Stop the service cleanly and checksum an archive of the full data root.
   Select the candidate and verify the populated root, private file bytes and
   retained Trash. Stop the candidate cleanly.
4. Verify the stopped archive checksum again. Restore to a new root in the same
   CT. Compare archive contents and numeric ownership before starting the exact
   prior executable on the restored root. Verify sign-in and file bytes, then
   stop it. Both roots and the private archive remain in the CT.

Retain the redacted evidence output and record the Proxmox release and exact
package identities. The coordinator verifies CT ownership again before any
teardown. This rehearsal does not exercise replacement of the guest, a second
CT, independent backup media, browser UI, or physical durability.
