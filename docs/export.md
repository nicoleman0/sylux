# Export an account to ordinary files

Stop Sylux first. Run as the operating-system user that owns the storage root,
using a Sylux binary that supports that root's schema:

```sh
sylux admin export --root /srv/sylux --handle alice --destination /srv/alice-export
```

The command takes Sylux's existing exclusive writer lock for the entire export.
It refuses to run while a server or another offline command holds that lock.
The lock is advisory: do not modify the root with other tools during export.

The destination must be absent or an empty directory. Its parent must exist.
It cannot be the storage root, inside it, or an ancestor of it. A destination
symlink is refused. Newly created directories have mode `0700`; files have
mode `0600`. Use a private destination and parent, with enough free space for
both the exported files and a temporary copy of metadata (including any WAL).
Existing directory permissions are retained.

Only the selected account's active tree is exported. Empty folders and current
file contents are included. Trash, old revisions and other accounts are excluded.
This is a plain-file exit path, not an installation backup: accounts, passwords,
permissions, history and application metadata are not exported. File timestamps
are those of export, not upload times.

Names are preserved exactly. Each entry is created exclusively on the actual
target filesystem. An unsupported name, changed spelling, or a collision
(including case or Unicode normalization aliases) stops the export. Existing
entries are never overwritten, and names are never silently changed. A target
that supports two distinct names receives both, even if another filesystem
would consider them aliases.

Every payload is checked against its recorded length and SHA-256 while copying
through a 64 KiB buffer. Metadata is copied to a temporary private directory
inside the destination before SQLite opens it, so SQLite cannot create or change
source journals or shared-memory files. No migrations or startup recovery run.
Reading may update filesystem access times. Temporary metadata is removed when
the command exits normally, including error exits; a killed process may leave a
`.sylux-export-*` directory inside the destination. Remove that private directory
once no export process is using it.

Success prints `export_complete files=N` and exits zero. Any failure exits
nonzero and reports its reason on stderr. Name errors include the quoted name.
A failure can leave directories, complete files and a partial or corrupt file in
the destination. Treat the entire destination as incomplete, remove it yourself,
and retry into an absent or empty destination after resolving the error. An
incomplete destination may contain a `.sylux-export-*` scratch directory with
metadata for every account, including credential verifiers. Do not share it;
remove the entire incomplete destination after stopping any export process. Do not
use a failed export as a backup. The source is never repaired by this command.

## Docker Compose

Use a release that includes `admin export`. From the installation directory,
stop the service and create an empty private directory on the host. The one-shot
container runs as UID/GID 10001 and needs write access to that directory.

```sh
docker compose stop || exit 1
RUNNING=$(docker compose ps --status running -q) || exit 1
test -z "$RUNNING" || exit 1
sudo install -d -o 10001 -g 10001 -m 0700 /srv/alice-export || exit 1
docker compose run --rm -T --volume /srv/alice-export:/export sylux \
  admin export --root /var/lib/sylux --handle alice --destination /export
```

Choose an empty destination outside the source root and replace `alice` with
the account handle. Compose supplies the usual source mount; export reads its
metadata and payloads without changing them, including WAL and SHM files.
The existing writer lock is opened read-write, so keep the source bind mount
writable even though export does not modify its contents.
The export is private account data. See [Docker beta install](docker-beta.md)
for the service configuration and restart procedure.
