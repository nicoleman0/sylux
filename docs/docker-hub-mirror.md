# Docker Hub release mirror

An owner-authorized release tag qualifies the Linux amd64 image, publishes
the GitHub Release, and pushes GHCR before the Docker Hub job runs. The mirror
checks the GHCR image against the image ID in the qualified release checksums,
copies the GHCR manifest by digest and verifies the Docker Hub tags anonymously.
The [Docker installation guide](docker-beta.md) uses the checksummed release
bundle as before.

## Owner setup

Create the public Docker Hub repository `nicoleman/sylux` before releasing.
In GitHub Actions, set these repository variables and secret together.

| Setting | Value |
| --- | --- |
| Variable `SYLUX_DOCKER_HUB_REPOSITORY` | `nicoleman/sylux` |
| Variable `SYLUX_DOCKER_HUB_USERNAME` | `nicoleman` for the owner's personal token |
| Secret `SYLUX_DOCKER_HUB_PUSH_TOKEN` | Docker Hub access token with read and write access, without delete access |

Give the token an expiry. A personal token inherits access to repositories the
account can access. For an organization-owned mirror, use a repository-scoped
organization token where available. [Docker Hub access tokens](https://docs.docker.com/security/access-tokens/)

Keep the release workflow as the sole writer. If using Docker Hub tag
immutability, protect version tags and leave `beta` and `latest` mutable.
[Docker Hub tag immutability](https://docs.docker.com/docker-hub/repos/manage/hub-images/immutable-tags/)

With all settings absent the mirror reports a skip. Partial settings fail the
mirror job after GitHub Release and GHCR publication. Pull requests test with
loopback registries. Branch release rehearsals skip publication.

## Recover a partial mirror

The job verifies the exact version first. It advances `beta` for beta releases
and `latest` for stable releases. If the alias already points to a verified
exact version that sorts higher on its channel, the job keeps it. This lets an
older retry finish its version tag without moving the alias backwards.

1. Check the release run summary for the expected digest, verified exact
   version and final alias status. GitHub Release and GHCR can be available
   even when the mirror fails. The exact Docker Hub version may also be
   available while the channel alias still points to an earlier release.
2. Correct the destination, token, visibility or registry error. Investigate
   an exact tag with a different digest, or an alias that does not match an
   exact tag on its channel, before retrying. The workflow refuses to replace
   a conflicting exact version.
3. As repository owner, re-run **only the Docker Hub image job** in the original
   tag run. This reuses the qualified checksum, the checksum-verified image
   ID and the GHCR version. It checks for writes that completed before the
   failure. GitHub supports specific job reruns for 30 days. Keep the original
   GitHub release and tag. [GitHub job reruns](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs#re-running-a-specific-job)

## Use the mirrored image with Compose

Use this on the [supported Docker host](docker-beta.md) with the Compose file
shipped for your release. For an upgrade, first follow the guide's
[backup and rollback procedure](docker-beta.md#5-upgrade-and-roll-back).

1. Select an exact version from the [GitHub Releases](https://github.com/nicoleman0/sylux/releases).
   From its Docker Hub image job summary, copy the digest on the `Verified
   index.docker.io/nicoleman/sylux:<version>` line. Download `SHA256SUMS`,
   `release.json` and `image-id.txt` from the matching GitHub release into the
   same directory. Run the following there with Bash, Docker and `jq` installed.
   [Docker digest pulls](https://docs.docker.com/reference/cli/docker/image/pull/#pull-an-image-by-digest-immutable-identifier)

   ```bash
   RELEASE='<exact version tag>'
   DIGEST='<sha256 digest from the verified version line>'
   PINNED="nicoleman/sylux@$DIGEST"
   printf '%s\n' "$DIGEST" | grep -Eq '^sha256:[0-9a-f]{64}$' || exit 1
   for asset in release.json image-id.txt; do
     grep -F "  $asset" SHA256SUMS | sha256sum --check --strict - || exit 1
   done
   EXPECTED_ID=$(cat image-id.txt) || exit 1
   printf '%s\n' "$EXPECTED_ID" | grep -Eq '^sha256:[0-9a-f]{64}$' || exit 1
   REVISION=$(jq -er --arg version "$RELEASE" --arg id "$EXPECTED_ID" '
     select(.version == $version and .platform == "linux/amd64" and .image_id == $id)
     | .source_commit | select(test("^[0-9a-f]{40}$"))' release.json) || exit 1

   REMOTE_ID=$(docker manifest inspect "$PINNED" | jq -er '.config.digest') || exit 1
   test "$REMOTE_ID" = "$EXPECTED_ID" || exit 1
   docker pull --platform linux/amd64 "$PINNED" || exit 1
   test "$(docker image inspect --format '{{.Os}}/{{.Architecture}} {{index .Config.Labels "org.opencontainers.image.version"}} {{index .Config.Labels "org.opencontainers.image.revision"}} {{index .Config.Labels "org.opencontainers.image.source"}}' "$PINNED")" = \
     "linux/amd64 $RELEASE $REVISION https://github.com/nicoleman0/sylux" || exit 1
   IMAGE_ID=$(docker image inspect --format '{{.Id}}' "$PINNED") || exit 1
   printf '%s\n' "$IMAGE_ID" | grep -Eq '^sha256:[0-9a-f]{64}$' || exit 1
   printf 'SYLUX_IMAGE=%s\n' "$IMAGE_ID"
   ```

2. Set `SYLUX_IMAGE` in the existing Compose project's `.env` to the local
   image ID printed above. Run `docker compose config -q` from that project.
   The shipped Compose file uses `pull_policy: never`. Keep the release bundle
   and pre-upgrade backup for rollback.
