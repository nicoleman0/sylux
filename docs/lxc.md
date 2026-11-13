# Native LXC installation scope

Docker is the primary beta installation. Native LXC is a secondary path for
operators who manage the guest OS, service account, TLS and systemd themselves.
There is no separately published native-binary artifact or qualified Proxmox
template in this beta. Running Docker inside LXC is not the reference deployment.

A native installation must use the same source revision as the selected release,
build the embedded UI before the Rust binary, and run one service identity against
one private local data root. Keep TLS keys outside that root. The guest needs a
compatible Linux amd64 userspace and the runtime libraries used by the build.
Do not assume a binary built on another distribution is portable.

LXC storage and isolation require their own checks: UID mapping and ownership,
file and directory synchronization, writer exclusion, service shutdown, backups
and restore. Keep the service private, verify TLS at both proxy hops, and set
`--public-origin` to the exact browser origin. Do not copy a proxy configuration
that disables backend certificate verification.

The Docker CI result does not qualify LXC storage, permissions or restore. Use the
[Docker guide](docker-beta.md) for the supported beta onboarding path until the
specific LXC environment has its own recorded evidence. Send environment details
and failures to hello@corvus-dev.com; do not include credentials or personal files.

## Recover administrator access

Use a binary that includes `admin reset-password`; stale binaries reject the
command. Recovery requires an existing administrator handle and completed browser
setup. It changes one administrator's password, enables that account and revokes
its sessions. It does not reopen setup, create accounts or repair storage.

Confirm the unit's `User`, `Group`, `ExecStart` and data root first. These examples
assume `sylux.service`, service account `sylux`, binary `/usr/local/bin/sylux` and
root `/srv/sylux`. Substitute the exact values from the guest; do not run recovery
as root or change ownership to bypass the service identity checks.

```sh
sudo systemctl show sylux.service -p User -p Group -p ExecStart
sudo systemctl stop sylux.service || exit 1
if sudo systemctl is-active --quiet sylux.service; then exit 1; fi
```

Make a stopped-service archive on a separate mounted filesystem as in the
[Docker backup procedure](docker-beta.md#4-stop-back-up-and-restore), using the
guest's actual data root. Omit the Docker stop/start commands. Keep numeric
ownership and the whole root, including secrets and SQLite siblings. Verify its
checksum before recovery:

```sh
sudo sha256sum -c /mnt/backup/sylux/backup.tar.sha256 || exit 1
sudo -v || exit 1
bash <<'RECOVERY' || exit 1
set +x
set +o history
set -o pipefail
trap 'unset sylux_password' EXIT
IFS= read -r -s -p 'New administrator password: ' sylux_password </dev/tty || exit 1
printf '\n' >/dev/tty
printf '%s' "$sylux_password" | sudo -n -u sylux -- /usr/local/bin/sylux \
  admin reset-password --root /srv/sylux \
  --handle '<existing administrator handle>' --password-stdin
RECOVERY
sudo systemctl start sylux.service || exit 1
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
the new credentials before another reset. This procedure still needs native LXC
qualification on the operator's storage and service configuration.
