# Docker beta install

This is a beta procedure for **one** Sylux instance on Ubuntu 24.04 x86_64, with a local ext4 data directory and SQLite. Use only a published release that includes every file below. Do not keep your only copy of important files in Sylux.

The tested path used a disposable Ubuntu 24.04 x86_64 runner, an ext4 bind mount, TLS on host loopback, restart and replacement, and a stopped-service archive restore. Physical storage durability, a separate physical client, upgrades, interruption recovery, and this Compose file have not been qualified. Native LXC is a separate installation path.

## 1. Get and verify a published release

Install Docker Engine with its Compose plugin, `curl`, `sha256sum`, and `openssl` on the Linux host. Pick a **published** release tag from [Sylux releases](https://github.com/nicoleman0/sylux/releases). Set `RELEASE` to that exact tag, then download all of its Docker assets into a new private directory:

```sh
RELEASE='<published release tag>'
mkdir -m 0700 sylux-install || exit 1
cd sylux-install || exit 1
BASE="https://github.com/nicoleman0/sylux/releases/download/${RELEASE}"
for asset in sylux-image.tar.gz SHA256SUMS image-id.txt compose.yaml .env.example docker-beta.md release.json; do
  curl --fail --location --show-error --output "$asset" "$BASE/$asset" || exit 1
done
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

The local image ID is a full `sha256:<64 hex digits>` value. `image-id.txt` records the build engine's ID; another Docker image store may report a different ID for the loaded image. Compose pins the locally inspected ID with `pull_policy: never`, so it neither builds nor requests an image from a registry. Keep the archive and checksum for rollback. The source Dockerfile can be built for development, but that is outside this prebuilt-image install.

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
cp .env.example .env
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
sudo sh -c 'sha256sum /mnt/backup/sylux/backup.tar > /mnt/backup/sylux/backup.tar.sha256' || exit 1
docker compose up -d || exit 1
```

Confirm the backup mount is a separate filesystem before archiving. If `SYLUX_DATA_DIR` differs from `/srv/sylux`, replace that source path in the archive command. Copy the archive and checksum off the host. Test a restore with disposable data before relying on this procedure. An archive listing and checksum do not prove storage durability.

To restore, stop Sylux, verify the checksum, and extract into a **new empty directory** on the same supported host filesystem. Never serve the old and restored roots together:

```sh
docker compose stop || exit 1
RUNNING=$(docker compose ps --status running -q) || exit 1
test -z "$RUNNING" || exit 1
sudo sha256sum -c /mnt/backup/sylux/backup.tar.sha256 || exit 1
sudo mkdir -m 0700 /srv/sylux-restore || exit 1
sudo chown 10001:10001 /srv/sylux-restore || exit 1
sudo tar --numeric-owner --same-owner --same-permissions \
  -C /srv/sylux-restore -xpf /mnt/backup/sylux/backup.tar || exit 1
```

Set `SYLUX_DATA_DIR=/srv/sylux-restore` in `.env`, then run `docker compose up -d`. Check sign-in, inventory, and byte-exact downloads before accepting the restore. Preserve the original root until the restore is verified. The documented CI exercise used a disposable archive; real operator backups remain unqualified.

## 5. Upgrade and roll back

Keep the current release archive, `.env`, and local image ID. Stop the service and make a verified pre-upgrade archive as in step 4 **without starting it again**. Download and verify all assets for the new release into a new directory as in step 1, then load and inspect its version tag. Copy the prior `.env` into the new directory, set `SYLUX_IMAGE` to the newly inspected local ID, run `docker compose config -q`, and start there with `docker compose up -d`. Check sign-in and known file downloads.

If the new image fails before it starts, restore the prior image ID in `.env` and start it against the unchanged root. If the new image starts or may have written to the root, stop it, restore the pre-upgrade archive into a new empty directory, set both the prior image ID and `SYLUX_DATA_DIR` in `.env`, and start. Do not run a previous image against a root that a newer image may have changed. Upgrade and schema rollback have not been qualified.

Send beta feedback and the host, Docker Engine/Compose versions, image ID, and a brief reproduction to **hello@corvus-dev.com**. Remove setup tokens, passwords, private keys, and file contents before sharing logs.

Docker reference: [Compose bind mounts](https://docs.docker.com/reference/compose-file/services/#volumes), [variable interpolation](https://docs.docker.com/compose/how-tos/environment-variables/variable-interpolation/), [`docker compose run`](https://docs.docker.com/reference/cli/docker/compose/run/), and [pull policy](https://docs.docker.com/reference/compose-file/services/#pull_policy).
