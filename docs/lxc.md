# Install Sylux in Proxmox LXC

Use one **unprivileged Debian 13 amd64** container with systemd and a
Proxmox-managed local LVM-thin root mounted as ext4 in the guest. The helper
installs a native binary and starts a loopback-only TLS service. Docker on an
Ubuntu host remains the [primary installation path](docker-beta.md).

The helper uses the community-scripts split between a Proxmox host entry point
and an unattended in-guest installer. It runs from reviewed local files against
a CT you provision and mark. The installer needs no network in the guest once
Debian prerequisites are present. Sylux maintains this helper in its own
repository. [Community Scripts contribution guide](https://github.com/community-scripts/ProxmoxVE/blob/42e82d9aadf0e6fa2507feec02dc5c2c6c029d2b/CONTRIBUTING.md).

## 1. Download the exact release

Choose a published tag with native assets. From its release record, get the full
source commit and SHA-256 of `SHA256SUMS-native`. The Proxmox host needs
`python3` for bundle validation. Download
into a new private directory:

```bash
VERSION='<published tag with native assets>'
SOURCE_SHA='<full 40-character release source commit>'
MANIFEST_SHA='<SHA-256 of SHA256SUMS-native>'
BUNDLE=$(mktemp -d)
BASE="https://github.com/nicoleman0/sylux/releases/download/$VERSION"
for file in SHA256SUMS-native install-sylux-lxc.sh "sylux-$VERSION-linux-amd64.tar.gz"; do
  curl -q --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --output "$BUNDLE/$file" "$BASE/$file" || exit 1
done
(
  cd "$BUNDLE" || exit 1
  printf '%s  SHA256SUMS-native\n' "$MANIFEST_SHA" | sha256sum --check || exit 1
  sha256sum --check SHA256SUMS-native
) || exit 1
```

The manifest covers the installer and archive. The archive contains the binary,
MIT license, third-party notices and `native-release.json`. Web notices ship
inside the embedded web assets at `/licenses/third-party.txt`. GitHub serves the
assets and their published digests under the same publisher identity. Obtain the
host helper from the same reviewed source revision, and review the host and
guest scripts before running them.

## 2. Provision the guest

As root on the chosen Proxmox node, select an unused CT ID, a cached Debian 13
amd64 appliance template and active local LVM-thin storage. Check the node's
`pveam list <template-storage>` and `pvesm status` for actual volume and storage
names. Choose a bridge on your trusted management network with DHCP and
access to Debian package mirrors.

```bash
CTID='<unused CT ID>'
TEMPLATE='<storage:vztmpl/debian-13-…_amd64.tar.zst>'
ROOT_STORAGE='<local LVM-thin storage ID>'
BRIDGE='<existing trusted Proxmox bridge>'
MARKER="sylux-$(openssl rand -hex 16)"
pct create "$CTID" "$TEMPLATE" --hostname sylux --ostype debian --arch amd64 \
  --unprivileged 1 --description "$MARKER" --rootfs "$ROOT_STORAGE:8" \
  --features nesting=1 --net0 "name=eth0,bridge=$BRIDGE,ip=dhcp" \
  --cores 2 --memory 1024 --onboot 0 || exit 1
pct start "$CTID" || exit 1
```

Keep the ownership marker. The host helper reads current and pending config and
requires matching settings, the marker, and stock unprivileged isolation.
Debian 13 uses systemd 257. Proxmox recommends nesting for newer systemd
versions because services can otherwise fail at boot. Keep `nesting=1` as
the only feature and omit custom LXC directives, devices and extra mounts.
Container IDs and storage names are local to the node. [Proxmox container
options and commands](https://github.com/proxmox/pve-docs/blob/0d8269d6c45caeffed7a6d71b695ad25217c4a4c/generated/pct.1-synopsis.adoc). [Proxmox systemd nesting check](https://github.com/proxmox/pve-container/blob/4ffd00b03858d3396bc8cd7bbbc6c5b01a3b4067/src/PVE/LXC/Setup/Base.pm#L662).

Inside the guest, install prerequisites and SSH from trusted Debian repositories.
Use `pct exec "$CTID" -- ip -4 -brief address show dev eth0` on the node to
find the guest IP. With `pct enter "$CTID"`, run the following as guest root.
Replace the public key placeholder with your workstation's public key before
running the block:

```bash
apt-get update && apt-get install --yes python3 openssl libgcc-s1 iproute2 \
  procps util-linux openssh-server || exit 1
adduser --disabled-password --gecos '' sylux-operator || exit 1
install -d -o sylux-operator -g sylux-operator -m 0700 /home/sylux-operator/.ssh || exit 1
printf '%s\n' '<your workstation public SSH key>' \
  > /home/sylux-operator/.ssh/authorized_keys || exit 1
chown sylux-operator:sylux-operator /home/sylux-operator/.ssh/authorized_keys || exit 1
chmod 0600 /home/sylux-operator/.ssh/authorized_keys || exit 1
systemctl enable --now ssh.service || exit 1
ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
```

Record the SSH host key fingerprint from the guest console and compare it
when first connecting from the workstation. Keep SSH access on the trusted network.
The installer checks the LXC UID map and guest release, and checks the ELF and
dynamic libraries before creating the service or data root.

## 3. Install and set up

From the reviewed source checkout on the Proxmox host, run:

```bash
bash deploy/lxc/ct/sylux.sh install "$CTID" "$MARKER" "$BUNDLE" "$VERSION" "$SOURCE_SHA" "$MANIFEST_SHA"
```

The helper verifies the package, pushes three files to a private directory in
the marked CT, and rechecks their digests before calling the guest installer.
It installs the version under `/opt/sylux/releases` and enables
`sylux.service`. The dedicated `sylux` account owns `/srv/sylux` with mode
0700. The root-owned TLS key under `/etc/sylux` is readable by the service group.
The installer refuses an existing service account or install path. If it
stops partway through, inspect guest state and preserve any initialized root
before recovery.

1. On the workstation, set `GUEST_IP` to the guest's DHCP address and run
   the tunnel below. Confirm the recorded host key if prompted. Keep this
   terminal open during browser use.

   ```bash
   ssh -N -o ExitOnForwardFailure=yes -o PreferredAuthentications=publickey \
     -L 127.0.0.1:3000:127.0.0.1:3000 "sylux-operator@$GUEST_IP"
   ```

2. Copy only `/etc/sylux/cert.pem` from the guest and trust it on the client
   using a private channel. The default certificate covers `127.0.0.1` and
   expires after one year. Keep the private key in the guest.
3. Open `https://127.0.0.1:3000` and select **First-time setup**. In a private
   terminal with recording disabled, use
   `pct exec "$CTID" -- cat /srv/sylux/secrets/setup.authorization`. Enter the
   one-use authorization in the setup form without capturing it in logs.
4. Sign in, upload a disposable file and download it to check the contents.

Keep the service private and bound to guest loopback. Keep TLS keys
separate from the data archive, and back them up separately if you need to
preserve client trust. Use `systemctl restart sylux.service` in the guest for a
service restart.

## Back up the stopped root

Run in the guest as root. Choose a new private directory on separate backup
storage, or copy the archive and its checksum off the CT before changing the
installation. Treat the archive as private. It includes credentials and files.

```bash
ARCHIVE_DIR='<new private archive directory>'
systemctl stop sylux.service || exit 1
test "$(systemctl show sylux.service -p ActiveState --value)" = inactive || exit 1
test "$(systemctl show sylux.service -p Result --value)" = success || exit 1
umask 077
mkdir "$ARCHIVE_DIR" || exit 1
tar --create --numeric-owner --file "$ARCHIVE_DIR/root.tar" --directory /srv/sylux . || exit 1
(cd "$ARCHIVE_DIR" && sha256sum root.tar > root.tar.sha256 && sha256sum --check root.tar.sha256) || exit 1
```

Keep the entire stopped root, numeric ownership, and exact matching release.
For an ordinary backup, start the service and check sign-in and downloads.

## Upgrade and restore

1. Download and verify the newer exact package. Back up the stopped root and
   keep the matching previous package.
2. Run the Proxmox helper with `upgrade` and the new bundle, tag, revision and
   digest. The installer requires a stopped service with its original unit
   command and no drop-ins. It stages a new release
   without initializing or replacing the data root. It leaves the service
   stopped so you can check the version selection before starting it.
3. Start `sylux.service` and check readiness, sign-in, inventory and a known
   download over trusted TLS. Keep the archive until you have verified recovery.

For rollback, stop the service and verify the archive checksum. Extract with
`tar --extract --numeric-owner --same-owner --same-permissions` into a new empty
root on suitable local storage, preserving the whole directory tree. Verify it
with `tar --compare --numeric-owner` before starting the prior release on the
restored root. Preserve the original root for inspection. Keep the same guest
service UID and GID. Use a drop-in such as
`/etc/systemd/system/sylux.service.d/restore.conf` with
`Environment=SYLUX_DATA_ROOT=/srv/sylux-restored`, an empty `ExecStart=`
to reset the command, and a copy of the original `ExecStart=` with
`/opt/sylux/releases/<prior-tag>/sylux` in place of the `current` path.
Put all three directives under `[Service]`. Restart after `systemctl daemon-reload`. Check
ownership, sign-in and file bytes before reopening access.

Keep running with the drop-in until you are ready to reconcile the paths.
Before retrying the same package or installing a later one, stop Sylux and
archive the restored root as it stands, including any writes since rollback.
Move the original `/srv/sylux` aside to a new private path on the same
filesystem, then rename the restored root to `/srv/sylux`. Keep the original
root, the pre-upgrade archive, and the new archive. Verify the selected
paths before moving them. Park the unused candidate release directory
at a fresh path outside `/opt/sylux/releases` so a retry can install its tag
again. Atomically repoint `/opt/sylux/current` to `releases/<prior-tag>` using
a fresh temporary symlink and `mv -Tf`. Move the restore drop-in out of the
unit search path, run `systemctl daemon-reload`, and verify the prior binary,
`/srv/sylux`, and absence of drop-ins. Start and verify the prior service on
that root first. Take a fresh stopped backup and repeat the normal upgrade
steps with the verified package after resolving the original failure.

Snapshot restore can affect public file and folder links. [Restrict recipient access](sharing.md#backup-and-restore) at the proxy or network before restoring.
The snapshot may revive a link revoked later. Keep access restricted through
owner review and revocation. Owners can review their own links. Keep the
restriction if owners cannot review.

## Qualification

The release workflow runs an offline Debian 13 amd64 container smoke test. It
exercises the exact package through certificate-verified first setup, upload,
download and restart. The live Proxmox driver additionally checks systemd boot,
guest UID mapping, filesystem ownership, populated upgrade and stopped archive
restore to a second root in the same CT. The measured beta.10 and stable
builds contained byte-identical executables. That comparison covers package
version replacement and data persistence. Changed-binary and schema upgrades
are outside its scope. See the [NIC-less live rehearsal](lxc-rehearsal.md)
for prerequisites, inputs and checks. These results apply to the tested releases and storage
configuration. Physical durability, guest replacement and other storage
backends are outside the rehearsal.

## Recover administrator access

Use a binary that includes `admin reset-password`; stale binaries reject the
command. Recovery requires an existing administrator handle and completed browser
setup. It changes one administrator's password, enables that account and revokes
its sessions. It does not reopen setup, create accounts or repair storage.

Confirm the unit's `User`, `Group`, `ExecStart` and data root first. These examples
assume `sylux.service`, service account `sylux`, binary `/opt/sylux/current/sylux` and
root `/srv/sylux`. Run the shell steps as guest root and the recovery command
as the service account via `runuser`. Substitute the exact values from the guest.

```sh
systemctl show sylux.service -p User -p Group -p ExecStart
systemctl stop sylux.service || exit 1
test "$(systemctl show sylux.service -p ActiveState --value)" = inactive || exit 1
```

Take a [stopped-service archive](#back-up-the-stopped-root), preserving the
whole root, including numeric ownership, SQLite siblings and secrets.

Verify the checksum:

```sh
(cd "$ARCHIVE_DIR" && sha256sum --check root.tar.sha256) || exit 1
bash <<'RECOVERY' || exit 1
set +x
set +o history
set -o pipefail
trap 'unset sylux_password' EXIT
IFS= read -r -s -p 'New administrator password: ' sylux_password </dev/tty || exit 1
printf '\n' >/dev/tty
printf '%s' "$sylux_password" | runuser -u sylux -- /opt/sylux/current/sylux \
  admin reset-password --root /srv/sylux \
  --handle '<existing administrator handle>' --password-stdin
RECOVERY
systemctl start sylux.service || exit 1
```

Replace the handle placeholder. Use a trusted terminal without session recording.
The Bash block disables tracing/history and hides input; the password goes only
to stdin. Do not pass it through arguments, environment variables or a file.
Passwords require 15–128 characters and preserve spaces. The command refuses
terminal stdin and invalid input. It requires the private root's existing owner,
binding, schema and integrity checks to pass, and it keeps the writer lock until
SQLite closes. Do not use `init` or edit SQLite when a precondition fails.

After restart, check new-password sign-in, rejection of the old password and old
sessions, closed setup, unchanged inventory and byte-exact downloads. A held lock
requires stopping the remaining writer. On an uncertain commit, restart and test
the new credentials before another reset. Verify recovery on the storage and service configuration you use.

## Schema upgrade

`serve` upgrades v1 and v2 roots to v3 and resumes an interrupted manifest
write. Take a complete stopped-service backup first; rollback needs that full
backup and its matching release. Offline payload verification and password
recovery require the upgrade to finish first and report `*_schema_upgrade_required_run_serve` for a v1 or v2 manifest.
See [Trash upgrade and recovery](trash.md#schema-upgrade-and-offline-admin) before
upgrading or using offline password recovery or payload verification.
