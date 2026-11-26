# Docker install

Use this procedure for **one** Sylux instance on Ubuntu 24.04 x86_64, with a local ext4 data directory and SQLite. Use a published release and the matching guide shipped with it. Do not keep your only copy of important files in Sylux.

The tested path used a disposable Ubuntu 24.04 x86_64 runner, an ext4 bind mount, TLS on host loopback, restart and replacement, and a stopped-service archive restore. See [the upgrade rehearsal](docker-upgrade-rehearsal.md) for the tested upgrade and rollback. The rehearsal covers only its pinned packages on a disposable runner; upgrades and this Compose file are not qualified for other environments. Physical storage durability, a separate physical client, and power interruption recovery remain outside its scope. Native LXC is a separate installation path.

## Docker quick start

Use an Ubuntu 24.04 x86_64 host running systemd, with `/srv` on local ext4 and TCP port 3000 free. The installer uses Ubuntu's Docker and Compose packages when Docker is absent. An existing installation needs a working local Docker Engine and the Compose v2 plugin. Download and review `install-sylux.sh` from a [published release](https://github.com/nicoleman0/sylux/releases) that includes it. Pass its exact tag, for example `v0.1.0` for stable or `v0.1.0-beta.10` for beta.

```sh
sudo bash ./install-sylux.sh '<published version tag>'
```

The installer displays the privileged changes and asks before proceeding. It prepares UID/GID 10001, the verified release and TLS files under `/opt/sylux`, and the private data directory `/srv/sylux`. Use `--yes` only after reviewing the plan if you need unattended installation. Checksums detect mismatched downloads. The release and checksum share the same publisher.

1. When the installer reports ready, export the public CA with `sudo cat /opt/sylux/tls/ca.crt > sylux-loopback.crt`. Import it into the browser device's trusted certificate store. The private TLS key stays on the host. Before the one-year expiry, [prepare a replacement loopback certificate](#2-prepare-the-host), keep the matching public trust certificate in `/opt/sylux/tls/ca.crt`, and update client trust.
2. Open `https://127.0.0.1:3000`. From another computer, use the [SSH local forward](#3-initialize-and-create-the-administrator) first. Choose **First-time setup**.
3. In a private terminal with recording disabled, run `sudo -u sylux cat /srv/sylux/secrets/setup.authorization`. Paste the one-use authorization into the browser setup form and create an administrator. Keep the authorization out of shell arguments, screenshots and logs.
4. Sign in, choose **Choose files** to upload a disposable file, then download it and check the contents.

Run the installer again with the same tag to resume an unsubmitted browser setup or start an installer-managed stopped service. To restart it yourself, use `sudo docker --host unix:///var/run/docker.sock compose --project-directory /opt/sylux restart sylux`. The backup and restore examples below assume the manual install. For the installer, use a root shell, run Compose from `/opt/sylux`, and retain that directory and the private TLS material alongside your recovery plan.

If the installer reports uncertain initialization or unfamiliar state, retain both directories and inspect the reported conflict before retrying. For a release change, take a backup and use the [manual upgrade procedure](#5-upgrade-and-roll-back). That moves the installation out of installer management; do not rerun the installer after upgrading. Releases without `install-sylux.sh` use the manual steps below.

### Recover an empty interrupted initialization

The installer refuses an `initializing` record. If init may have written data, retain both directories and investigate; do not reset it. Only reset after stopping all installer and init activity, confirming `/srv/sylux` is empty and owned by UID/GID 10001 with mode 700, and confirming there are no Compose project containers, including stopped or one-off containers. In a root shell, run this guarded reset. It holds the installer lock and changes only the phase record; it removes no data, accounts, images, or containers.

```bash
(
  flock -n 9 || exit 1
  test "$(stat -c '%u:%g:%a' /opt/sylux)" = 0:0:700 || exit 1
  test -d /opt/sylux && test -d /srv/sylux || exit 1
  test ! -L /opt/sylux && test ! -L /srv/sylux || exit 1
  test "$(stat -c '%u:%g:%a' /srv/sylux)" = 10001:10001:700 || exit 1
  test "$(cat /opt/sylux/phase)" = initializing || exit 1
  contents=$(find /srv/sylux -mindepth 1 -maxdepth 1 -print -quit) || exit 1
  test -z "$contents" || exit 1
  containers=$(docker --host unix:///var/run/docker.sock ps -aq --filter label=com.docker.compose.project=sylux) || exit 1
  test -z "$containers" || exit 1
  printf 'prepared\n' > /opt/sylux/phase.new
  mv -T /opt/sylux/phase.new /opt/sylux/phase
) 9</opt
```

If any check fails, leave the state intact. After a successful reset, rerun the original installer tag. Do not use this procedure to reset an initialized installation or recover credentials.

## 1. Get and verify a published release

Install Docker Engine with its Compose plugin, `curl`, `sha256sum`, and `openssl` on the Linux host. Pick a **published** release tag from [Sylux releases](https://github.com/nicoleman0/sylux/releases). Set `RELEASE` to that exact tag, then download all of its Docker assets into a new private directory:

```sh
RELEASE='<published release tag>'
mkdir -m 0700 sylux-install || exit 1
cd sylux-install || exit 1
BASE="https://github.com/nicoleman0/sylux/releases/download/${RELEASE}"
for asset in sylux-image.tar.gz SHA256SUMS image-id.txt compose.yaml env.example docker-beta.md release.json; do
  curl --fail --location --show-error --output "$asset" "$BASE/$asset" || exit 1
done
if grep -q '  install-sylux\.sh$' SHA256SUMS; then
  curl --fail --location --show-error --output install-sylux.sh "$BASE/install-sylux.sh" || exit 1
fi
sha256sum -c SHA256SUMS || exit 1
```

Stop if a download or checksum fails. `SHA256SUMS` covers every other listed asset. The checksum detects a mismatched download; it does not provide a separate publisher signature. Check `release.json` for the selected version, `linux/amd64` platform, source revision, and build image ID. Load the archive and inspect the **local** image under the release tag:

```sh
docker load -i sylux-image.tar.gz || exit 1
IMAGE_ID=$(docker image inspect "sylux:$RELEASE" --format '{{.Id}}') || exit 1
printf '%s\n' "$IMAGE_ID" | grep -Eq '^sha256:[a-f0-9]{64}$' || exit 1
test "$(docker image inspect "sylux:$RELEASE" --format '{{.Os}}/{{.Architecture}}')" = linux/amd64 || exit 1
test "$(docker image inspect "sylux:$RELEASE" --format '{{index .Config.Labels "org.opencontainers.image.version"}}')" = "$RELEASE" || exit 1
```

The local image ID is a full `sha256:<64 hex digits>` value. `image-id.txt` records the build engine's ID; another Docker image store may report a different ID for the loaded image. Compose pins the locally inspected ID with `pull_policy: never`, so it neither builds nor requests an image from a registry. Keep the archive and checksum for rollback. The source Dockerfile can be built for development, but that is outside this prebuilt-image install. The release workflow also pushes the exact release tag to `ghcr.io/nicoleman0/sylux`. Stable releases update `latest` and beta releases update `beta`. This procedure uses the archive from the GitHub release.

## 2. Prepare the host

Use a local ext4 filesystem for `/srv/sylux`. Do not use NFS, CIFS, Docker Desktop, or a second Sylux instance on the same root. Check that UID and GID 10001 are free before creating the service identity:

```sh
getent passwd 10001
getent group 10001
sudo groupadd --system --gid 10001 sylux
sudo useradd --system --uid 10001 --gid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin sylux
sudo install -d -o 10001 -g 10001 -m 0700 /srv/sylux /etc/sylux/tls
findmnt -T /srv/sylux -o TARGET,SOURCE,FSTYPE,OPTIONS
```

The first two commands should find no existing identity. If either does, stop and resolve the numeric ID conflict. The data root must be empty before `init`. Keep the whole root, including SQLite WAL files, secrets, payloads, and locks, on one mount.

Install a certificate and private key for `127.0.0.1`, with **IP subject alternative name `127.0.0.1`**, into `/etc/sylux/tls/cert.pem` and `/etc/sylux/tls/key.pem`. The certificate issuer must be trusted by every browser used. For a disposable trial, this creates a short-lived self-signed certificate:

```sh
sudo -u sylux openssl req -x509 -newkey rsa:3072 -sha256 -days 30 -noenc \
  -subj '/CN=127.0.0.1' -addext 'subjectAltName=IP:127.0.0.1' \
  -keyout /etc/sylux/tls/key.pem -out /etc/sylux/tls/cert.pem
sudo chmod 0600 /etc/sylux/tls/key.pem
sudo chmod 0644 /etc/sylux/tls/cert.pem
```

Install and trust `cert.pem` on each client through its operating system or browser certificate store before signing in. A self-signed certificate needs explicit client trust; a private CA can issue the same IP SAN instead. On mobile devices, importing a certificate may require a separate full-trust setting. Do not bypass browser certificate errors. Keep `key.pem` on the host and out of the data root and backups shared with clients.

## 3. Initialize and create the administrator

Create `.env` from the verified image ID. Keep the default paths unless you have prepared equivalent absolute host paths:

```sh
cp env.example .env
chmod 0600 .env
sed -i "s|^SYLUX_IMAGE=.*|SYLUX_IMAGE=$IMAGE_ID|" .env
docker compose config -q || exit 1
docker compose run --rm sylux init --root /var/lib/sylux || exit 1
docker compose up -d || exit 1
docker compose ps
```

`init` refuses an existing root; do not repeat it after setup. Compose bind mounts have `create_host_path: false`, so a missing host directory fails instead of being made silently. The container runs as UID/GID 10001 with a read-only root filesystem; only the data bind mount is writable. TLS is mounted read-only.

The listener is `https://127.0.0.1:3000` on the **host loopback**. Host networking does not publish it to the LAN. In a trusted local terminal, read the one-use setup authorization and enter it in the browser's setup page:

```sh
sudo -u sylux cat /srv/sylux/secrets/setup.authorization
```

Do not put this authorization in `.env`, shell arguments, screenshots, or logs. Create the administrator account in the setup page. Test sign-in, an upload, and a download before adding important files.

For a desktop on another machine, connect through SSH local forwarding and open the same URL on that desktop:

```sh
ssh -N -L 127.0.0.1:3000:127.0.0.1:3000 operator@private-host
```

The SSH host must be reachable privately and the browser must trust the TLS certificate for `127.0.0.1`. A mobile client needs an SSH app that provides a local forward to its browser, plus certificate trust on that device. This route was tested only from a separate container network namespace, not a physical desktop or phone. If forwarding is unavailable, use a reviewed private reverse proxy with a matching hostname certificate and a configured `--public-origin`; that deployment is outside this Compose example.

### Enable WebDAV

For the stock configuration, create `compose.override.yaml` next to `compose.yaml`:

```yaml
services:
  sylux:
    command: ["serve", "--root", "/var/lib/sylux", "--listen", "127.0.0.1:3000", "--tls-cert", "/run/sylux/tls/cert.pem", "--tls-key", "/run/sylux/tls/key.pem", "--webdav"]
```

For a customized service, keep its full command and append `--webdav`. Apply the change with `docker compose config -q && docker compose up -d`. See [WebDAV setup and compatibility](webdav.md) for client access.

## Restrict share access before restoring

Before restoring, restrict recipient access at the proxy or network. An unexpired share link revoked after the snapshot can become usable again. Keep recipient access restricted through owner review and revocation: each owner can inspect and revoke their own links, but there is no administrator-wide inventory. If owners cannot review, retain the restriction.

## Back up while serving

Use a release that includes `admin backup`. Run as the Sylux service identity.
Choose a private directory on a separate local filesystem. The backup includes
file contents, account and session data, the installation keys and instance
settings. Keep it as private as the live root.

With the default install paths, create a backup under `/mnt/backup/sylux`.
Use a new name for each backup. If you changed `SYLUX_DATA_DIR`, replace
`/srv/sylux` with the actual path. The service must be running for this
read-only mount so SQLite WAL sidecars are available. For a stopped instance,
use the [stopped-service archive](#4-stop-back-up-and-restore) instead.

```sh
findmnt -T /mnt/backup -o TARGET,SOURCE,FSTYPE,OPTIONS || exit 1
sudo install -d -o 10001 -g 10001 -m 0700 /mnt/backup/sylux || exit 1
docker run --rm --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges --user 10001:10001 \
  --mount type=bind,src=/srv/sylux,dst=/live,readonly \
  --mount type=bind,src=/mnt/backup/sylux,dst=/backups \
  "$IMAGE_ID" admin backup --root /live --destination /backups/run-001 || exit 1
```

For a native install, run `sylux admin backup --root ROOT --destination DIRECTORY`
as the service user. The parent must exist and the destination must be new or
empty. Use a parent outside the live root, with no other writable mount of the
live tree under it. A native or writable-mount backup opens SQLite read-only
but may create or update SQLite WAL sidecar files.

Exit 0 and `backup_complete payloads=N` mean Sylux checked the SQLite snapshot
and the copied bytes, then ran startup reconciliation on the copy. The count
includes any complete bodies awaiting upload commit. Use only the named output
of a successful command. An interruption can leave a private sibling directory
named `.sylux-backup-incomplete-*`. Discard it after confirming the command has
stopped. A failed command leaves live database data, payloads and secrets
unchanged and publishes no valid backup.

The metadata records one SQLite snapshot taken near the start of the command.
Files committed and present at that point, including retained Trash contents,
are copied with their recorded lengths and SHA-256 digests. An upload still
receiving bytes restarts from zero on restore. Instance name and appearance are
read separately from SQLite. If permanent removal unlinks a required payload
before it can be copied, the command fails and leaves the destination
unpublished. Retry with a fresh destination after removal finishes.

### Restore an online backup

[Restrict recipient access](#restrict-share-access-before-restoring) before starting the restored copy.

Use the same release, UID/GID and TLS setup. Stop the live service before
starting the restored copy. Keep the backup and the original root private and
intact. Copy the completed backup into a new empty directory on the supported
host filesystem.

```sh
docker compose stop || exit 1
RUNNING=$(docker compose ps --status running -q) || exit 1
test -z "$RUNNING" || exit 1
sudo mkdir -m 0700 /srv/sylux-restore || exit 1
sudo cp -a /mnt/backup/sylux/run-001/. /srv/sylux-restore/ || exit 1
```

Set `SYLUX_DATA_DIR=/srv/sylux-restore` in `.env`. Start with `docker compose up -d`,
check that the loopback listener is ready, sign in and compare inventory and
known downloads. The startup process reconciles the restored root before serving.
Keep the [stopped-service archive procedure](#4-stop-back-up-and-restore) for
pre-upgrade rollback and offline administrator recovery.

The online procedure was exercised with disposable data in a Linux arm64
container, including a held upload, startup reconciliation and byte-exact
downloads. The documented Ubuntu x86_64 install and its stopped-service
archive have separate verification. Online backup has no qualification for
power loss, production storage, or upgrades.

## 4. Stop, back up, and restore

Use `docker compose stop` for a clean stop and `docker compose up -d` to start again. `docker compose ps` shows whether it is running. Keep the data directory when replacing a container.

For a consistent archive, stop the service first. Choose a **separate mounted filesystem** for `/mnt/backup`; keep its archive private because it contains account data and file contents:

```sh
docker compose stop || exit 1
docker compose ps
RUNNING=$(docker compose ps --status running -q) || exit 1
test -z "$RUNNING" || exit 1
findmnt -T /mnt/backup -o TARGET,SOURCE,FSTYPE,OPTIONS || exit 1
sudo install -d -m 0700 /mnt/backup/sylux || exit 1
sudo tar --numeric-owner -C /srv/sylux -cpf /mnt/backup/sylux/backup.tar . || exit 1
sudo tar -tf /mnt/backup/sylux/backup.tar >/dev/null || exit 1
sudo sh -c 'cd /mnt/backup/sylux && sha256sum backup.tar > backup.tar.sha256' || exit 1
docker compose up -d || exit 1
```

Confirm the backup mount is a separate filesystem before archiving. If `SYLUX_DATA_DIR` differs from `/srv/sylux`, replace that source path in the archive command. Copy the archive and checksum off the host. Test a restore with disposable data before relying on this procedure. An archive listing and checksum do not prove storage durability.

[Restrict recipient access](#restrict-share-access-before-restoring) before starting the restored copy.

To restore, stop Sylux, verify the checksum, and extract into a **new empty directory** on the same supported host filesystem. Never serve the old and restored roots together:

```sh
docker compose stop || exit 1
RUNNING=$(docker compose ps --status running -q) || exit 1
test -z "$RUNNING" || exit 1
sudo sh -c 'cd /mnt/backup/sylux && sha256sum -c backup.tar.sha256' || exit 1
sudo mkdir -m 0700 /srv/sylux-restore || exit 1
sudo chown 10001:10001 /srv/sylux-restore || exit 1
sudo tar --numeric-owner --same-owner --same-permissions \
  -C /srv/sylux-restore -xpf /mnt/backup/sylux/backup.tar || exit 1
```

Set `SYLUX_DATA_DIR=/srv/sylux-restore` in `.env`, then run `docker compose up -d`. Check sign-in, inventory, and byte-exact downloads before accepting the restore. Preserve the original root until the restore is verified. The documented CI exercise used a disposable archive; real operator backups remain unqualified.

## Recover administrator access

Recovery requires host access and an existing administrator handle. It resets that
account's password, enables it and revokes its sessions. It does not create an
account or reopen browser setup. Other accounts and files keep their state.

Use an image built from a release that includes `admin reset-password`. Older
images reject the command. Check the selected release before proceeding; do not
use `init`, edit SQLite or change the installation identity to work around that.

Stop Sylux and make the archive in step 4, **omitting the restart**. Verify the
archive checksum before changing credentials. Run from the installation directory
with its verified `.env` and Compose file. The one-shot container uses the same
UID/GID 10001 and `/var/lib/sylux` bind mount as the service:

```sh
docker compose stop || exit 1
RUNNING=$(docker compose ps --status running -q) || exit 1
test -z "$RUNNING" || exit 1
sudo sh -c 'cd /mnt/backup/sylux && sha256sum -c backup.tar.sha256' || exit 1
bash <<'RECOVERY' || exit 1
set +x
set +o history
set -o pipefail
trap 'unset sylux_password' EXIT
IFS= read -r -s -p 'New administrator password: ' sylux_password </dev/tty || exit 1
printf '\n' >/dev/tty
printf '%s' "$sylux_password" | docker compose run --rm -T sylux \
  admin reset-password --root /var/lib/sylux \
  --handle '<existing administrator handle>' --password-stdin
RECOVERY
docker compose up -d || exit 1
```

Replace the handle placeholder before running. Use a trusted local terminal with
session recording disabled. The Bash block disables tracing and history, reads
without echo and sends the password only through stdin. Do not put a password in
a command, environment variable or file. Passwords require 15–128 characters;
spaces are significant. The command refuses terminal stdin, multiple lines,
invalid UTF-8 and unsupported or damaged installations. It does not migrate or
repair the database and never starts a listener.

After restart, sign in with the new password. Confirm the old password and an
existing session fail, setup remains closed, and inventory and known downloads
match. A held writer lock means the service still needs to stop. If the command
reports an uncertain commit, restart and test the new credentials before another
reset; do not retry automatically.

## Verify stored payloads

This checks that every stored payload still matches the length and SHA-256
recorded for it. Stop Sylux first. The command refuses to run while the service
holds the root, and it does not write to payloads or metadata.

```sh
docker compose stop || exit 1
RUNNING=$(docker compose ps --status running -q) || exit 1
test -z "$RUNNING" || exit 1
docker compose run --rm -T sylux admin verify-payloads --root /var/lib/sylux
```

Use an image built from a release that includes `admin verify-payloads`.

- Exit 0: every payload matched. Output ends with
  `payload_verification checked=N verified=N damaged=0`.
- Exit 1: at least one payload is damaged, or the command could not run. Each
  damaged payload prints `damaged blob=<blob id> reason=<reason>`. Reasons are
  `missing`, `length_mismatch` (truncated or extended), `digest_mismatch`
  (same length, different bytes), `not_private_file` and `unreadable`.
- Output has blob IDs and counts only: no paths, file names or account handles.
- Run time is one full read of all payloads.

Limits: it reports and never repairs. A pass means the bytes matched at the time
of the run. It says nothing about later damage, power-loss durability or the
backup. Restore damaged payloads from a verified archive (step 4). Payload
files with no metadata row are not checked here; startup reconciliation covers
them. The command leaves any existing `-wal` unchanged and may create an empty
`-wal` and a `-shm` in `metadata/`.

## Export an account

For a plain-file copy of one account, stop Sylux and follow the
[Docker Compose export procedure](export.md#docker-compose).

## 5. Upgrade and roll back

Keep the current release archive, `.env`, and local image ID. Stop the service and make a verified pre-upgrade archive as in step 4 **without starting it again**. Download and verify all assets for the new release into a new directory as in step 1, then load and inspect its version tag. Copy the prior `.env` into the new directory, set `SYLUX_IMAGE` to the newly inspected local ID, run `docker compose config -q`, and start there with `docker compose up -d`. Check sign-in and known file downloads.

If the new image fails before it starts, restore the prior image ID in `.env`
and start it against the unchanged root. If the new image starts or may have
written to the root, use the pre-upgrade archive and its matching release:

1. [Restrict recipient access](#restrict-share-access-before-restoring) and
   confirm the upgraded service has stopped.
2. Verify the archive checksum and the prior release bundle. Restore into a
   new empty directory as in [step 4](#4-stop-back-up-and-restore). Preserve the
   upgraded root until recovery is verified.
3. Use the prior release's Compose file and set the matching immutable local
   image ID and recovered `SYLUX_DATA_DIR` in its `.env`. Validate the
   configuration with `docker compose config -q` and start the prior release.
4. Check fresh sign-ins, expected account and recipient access, known downloads,
   Trash, and working and revoked links against your pre-upgrade records.
   Keep the restriction in place until owners review restored access.

The [disposable rehearsal](docker-upgrade-rehearsal.md) exercises this
fresh-root procedure for beta.9 after beta.10. Its scope is the pinned beta
packages and the disposable Ubuntu/ext4 setup.

Send feedback and the host, Docker Engine/Compose versions, image ID, and a brief reproduction to **hello@corvus-dev.com**. Remove setup tokens, passwords, private keys, and file contents before sharing logs.

Docker reference: [Compose bind mounts](https://docs.docker.com/reference/compose-file/services/#volumes), [variable interpolation](https://docs.docker.com/compose/how-tos/environment-variables/variable-interpolation/), [`docker compose run`](https://docs.docker.com/reference/cli/docker/compose/run/), and [pull policy](https://docs.docker.com/reference/compose-file/services/#pull_policy).

## Schema upgrade

`serve` upgrades v1 and v2 roots to v3 and resumes an interrupted manifest
write. Take a complete stopped-service backup first; rollback needs that full
backup and its matching release. Offline payload verification and password
recovery require the upgrade to finish first and report `*_schema_upgrade_required_run_serve` for a v1 or v2 manifest.
See [Trash upgrade and recovery](trash.md#schema-upgrade-and-offline-admin) before
upgrading or using offline password recovery or payload verification.
