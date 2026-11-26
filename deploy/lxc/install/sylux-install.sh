#!/usr/bin/env bash
set +x
set -Eeuo pipefail
umask 077

fail() { printf 'Sylux LXC: %s\n' "$*" >&2; exit 1; }

check_upgrade_unit() {
  local property actual
  # systemctl reports the variable before expansion by the service manager.
  # shellcheck disable=SC2016
  local expected='/opt/sylux/current/sylux serve --root ${SYLUX_DATA_ROOT} --listen 127.0.0.1:3000 --tls-cert /etc/sylux/cert.pem --tls-key /etc/sylux/key.pem'
  local start="{ path=${expected%% *} ; argv[]=$expected ; ignore_errors=no ; "
  local status_fields='^start_time=\[[^{}]+\] ; stop_time=\[[^{}]+\] ; pid=[0-9]+ ; code=[^{}]+ ; status=[^{}]+ \}$'
  for property in DropInPaths FragmentPath NeedDaemonReload ExecStart ActiveState; do
    actual=$(systemctl show sylux.service -p "$property" --value) ||
      fail "Could not inspect sylux.service ($property)."
    case "$property" in
      DropInPaths)
        [[ -z "$actual" ]] || fail 'Remove or reconcile service drop-ins before upgrading.' ;;
      FragmentPath)
        [[ "$actual" == "$unit" ]] || fail 'Reconcile the service unit override before upgrading.' ;;
      NeedDaemonReload)
        [[ "$actual" == no ]] || fail 'Reload systemd and reconcile any unit changes before upgrading.' ;;
      ExecStart)
        [[ "$actual" == "$start"* && "${actual#"$start"}" =~ $status_fields ]] ||
          fail 'The effective service command is unfamiliar; reconcile it before upgrading.' ;;
      ActiveState)
        [[ "$actual" == inactive ]] ||
          fail 'Stop the service, reset any failed state with systemctl reset-failed sylux.service, and take a complete archive before upgrading.' ;;
    esac
  done
}

command=${1:-}
[[ $# == 5 && "$command" =~ ^(verify|install|upgrade)$ ]] ||
  fail 'Usage: install-sylux-lxc.sh verify|install|upgrade BUNDLE VERSION SOURCE_SHA MANIFEST_SHA256'
bundle=$2 version=$3 revision=$4 digest=$5
[[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-beta\.[1-9][0-9]*)?$ && "$revision" =~ ^[0-9a-f]{40}$ &&
   "$digest" =~ ^[0-9a-f]{64}$ ]] || fail 'Provide an exact tag, full source SHA and native manifest SHA-256.'
archive="sylux-$version-linux-amd64.tar.gz"

verify_bundle() {
  python3 - "$bundle" "$archive" "$version" "$revision" "$digest" "${1:-}" <<'PY'
import hashlib
import json
from pathlib import Path
import re
import sys
import tarfile

directory, archive, version, revision, digest, extract = sys.argv[1:]
directory = Path(directory)
names = {archive, "install-sylux-lxc.sh"}
required = {"sylux", "LICENSE", "native-release.json", "third-party", "third-party/index.json"}


def demand(condition):
    if not condition:
        raise ValueError()


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


try:
    files = list(directory.iterdir())
    demand({p.name for p in files} == names | {"SHA256SUMS-native"})
    demand(all(p.is_file() and not p.is_symlink() and p.stat().st_size for p in files))
    manifest = directory / "SHA256SUMS-native"
    demand(sha256(manifest) == digest)
    expected = "".join(f"{sha256(directory / name)}  {name}\n" for name in sorted(names))
    demand(manifest.read_text(encoding="ascii") == expected)

    with tarfile.open(directory / archive, "r:gz") as package:
        members = package.getmembers()
        entries = {member.name: member for member in members}
        demand(len(entries) == len(members) and required <= entries.keys())
        for name, member in entries.items():
            demand(name in required or name.startswith("third-party/"))
            demand(all(part not in ("", ".", "..") for part in name.split("/")))
            demand(member.isdir() if name == "third-party" else (
                member.isreg() or (name.startswith("third-party/") and member.isdir())
            ))
            demand(not member.mode & 0o7022)

        metadata = json.load(package.extractfile("native-release.json"))
        demand(metadata["version"] == version and metadata["source_commit"] == revision)
        demand(metadata["platform"] == "linux/amd64")
        demand(metadata["source_url"] == f"https://github.com/nicoleman0/sylux/tree/{revision}")
        demand(re.fullmatch(r"sha256:[0-9a-f]{64}", metadata["image_id"]))
        binary = package.extractfile("sylux").read(20)
        demand(binary[:6] == b"\x7fELF\x02\x01" and binary[18:20] == b"\x3e\x00")
        demand(entries["sylux"].mode == 0o755 and entries["LICENSE"].size > 0)
        demand(entries["third-party/index.json"].isreg())
        demand(any(name.startswith("third-party/") and name.count("/") > 1 and member.isreg()
                   for name, member in entries.items()))
        if extract:
            package.extractall(extract, filter="data")
except (OSError, ValueError, KeyError, TypeError, AttributeError, tarfile.TarError):
    sys.exit("Sylux LXC: native asset checksum, metadata or archive validation failed.")
PY
}

if [[ "$command" == verify ]]; then
  verify_bundle
  printf 'Verified %s at %s (native manifest %s)\n' "$version" "$revision" "$digest"
  exit
fi

[[ $EUID == 0 ]] || fail 'Run as root inside the guest.'
[[ "$(systemd-detect-virt --container)" == lxc && "$(ps -p 1 -o comm=)" == systemd ]] ||
  fail 'Requires a running systemd LXC guest.'
awk '$1 == 0 && $2 != 0 && $3 > 0 { mapped = 1 } END { exit !mapped }' /proc/1/uid_map ||
  fail 'Requires an unprivileged LXC guest.'
# shellcheck source=/dev/null
source /etc/os-release
[[ "${ID:-}/${VERSION_ID:-}/$(dpkg --print-architecture)" == debian/13/amd64 ]] ||
  fail 'Requires Debian 13 amd64.'

prefix=/opt/sylux
data=/srv/sylux
tls=/etc/sylux
unit=/etc/systemd/system/sylux.service
new_release="$prefix/releases/$version"
unused_path() { [[ ! -e "$1" && ! -L "$1" ]]; }

exec 9>/run/lock/sylux-native.lock
flock -n 9 || fail 'Another installer is running.'
stage=$(mktemp -d /var/tmp/sylux-native.XXXXXXXX)
trap 'rm -rf -- "$stage"' EXIT
mkdir "$stage/bundle" "$stage/package"
for file in SHA256SUMS-native install-sylux-lxc.sh "$archive"; do
  [[ -f "$bundle/$file" && ! -L "$bundle/$file" ]] || fail 'Expected regular staged assets.'
  cp -- "$bundle/$file" "$stage/bundle/"
done
bundle="$stage/bundle"
verify_bundle "$stage/package"
# The loader reports a missing runtime library before any persistent change.
dependencies=$(ldd "$stage/package/sylux") || fail 'The native binary could not be inspected.'
[[ "$dependencies" != *'not found'* ]] || fail 'Install the Debian runtime prerequisites first.'

if [[ "$command" == install ]]; then
  for target in "$prefix" "$data" "$tls" "$unit"; do
    unused_path "$target" || fail 'An installation path already exists. Inspect it before retrying.'
  done
  [[ "$(systemctl show sylux.service -p LoadState --value)" == not-found ]] ||
    fail 'An existing Sylux systemd unit was found.'
  if getent passwd sylux >/dev/null || getent group sylux >/dev/null; then
    fail 'The Sylux service identity already exists.'
  fi
  openssl req -x509 -newkey rsa:3072 -sha256 -days 365 -noenc \
    -subj /CN=127.0.0.1 -addext subjectAltName=IP:127.0.0.1 \
    -keyout "$stage/key.pem" -out "$stage/cert.pem" 2>/dev/null
  groupadd --system sylux
  useradd --system --gid sylux --no-create-home --home-dir "$data" --shell /usr/sbin/nologin sylux
  install -d -m 0755 "$prefix" "$prefix/releases"
  install -d -o sylux -g sylux -m 0700 "$data"
  install -d -o root -g sylux -m 0750 "$tls"
  install -o root -g sylux -m 0640 "$stage/key.pem" "$tls/key.pem"
  install -o root -g root -m 0644 "$stage/cert.pem" "$tls/cert.pem"
else
  [[ -f "$unit" && ! -L "$unit" && -L "$prefix/current" && -d "$prefix/current" &&
     -f "$data/installation.json" && ! -L "$data" && -d "$tls" && ! -L "$tls" ]] ||
    fail 'Expected an installer-managed native installation.'
  check_upgrade_unit
  installed=$(readlink "$prefix/current")
  [[ "$installed" =~ ^releases/v[0-9]+\.[0-9]+\.[0-9]+(-beta\.[1-9][0-9]*)?$ ]] ||
    fail 'Current release link is unfamiliar.'
  python3 - "${installed#releases/}" "$version" <<'PYTHON'
import sys


def key(version):
    base, _, beta = version[1:].partition("-beta.")
    return tuple(map(int, base.split("."))), not beta, int(beta or "0")


if key(sys.argv[1]) >= key(sys.argv[2]):
    sys.exit("Sylux LXC: upgrade requires a newer release.")
PYTHON
  if ! unused_path "$new_release" || ! unused_path "$prefix/.next"; then
    fail 'The selected release or an interrupted switch already exists.'
  fi
fi

# The staged copy initially has root-only access. Publish the complete version by rename.
release_stage=$(mktemp -d "$prefix/releases/.stage.XXXXXXXX")
trap 'rm -rf -- "$stage" "$release_stage"' EXIT
cp -a -- "$stage/package/." "$release_stage/"
chmod 0755 "$release_stage"
mv -T -- "$release_stage" "$new_release"
ln -s "releases/$version" "$prefix/.next"
mv -Tf -- "$prefix/.next" "$prefix/current"

if [[ "$command" == install ]]; then
  runuser -u sylux -- "$prefix/current/sylux" init --root "$data"
  cat > "$unit" <<'UNIT'
[Unit]
Description=Sylux file server
After=network.target

[Service]
Type=exec
User=sylux
Group=sylux
UMask=0077
Environment=SYLUX_DATA_ROOT=/srv/sylux
ExecStart=/opt/sylux/current/sylux serve --root ${SYLUX_DATA_ROOT} --listen 127.0.0.1:3000 --tls-cert /etc/sylux/cert.pem --tls-key /etc/sylux/key.pem
Restart=on-failure
NoNewPrivileges=yes

[Install]
WantedBy=multi-user.target
UNIT
  chmod 0644 "$unit"
  systemctl daemon-reload
  systemctl enable --now sylux.service
  printf 'Installed %s. Connect to https://127.0.0.1:3000 inside the guest.\n' "$version"
else
  printf 'Selected %s. Start sylux.service and check certificate-verified readiness.\n' "$version"
fi
