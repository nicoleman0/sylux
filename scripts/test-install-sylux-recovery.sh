#!/usr/bin/env bash
set -Eeuo pipefail
(( EUID == 0 )) || { echo 'Run as root in a disposable Linux test environment.' >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
chmod 0711 "$fixture"
umask 077
install_dir="$fixture/opt/sylux"
data_dir="$fixture/srv/sylux"
version=v0.1.0
revision=0123456789abcdef0123456789abcdef01234567
image_id=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
mkdir "$fixture"/{opt,srv,bundle}
# A default ACL can override the installer's umask on file creation.
setfacl -d -m u::rwx,g::rwx,o::rwx "$fixture/opt"
cp "$root/scripts/install-sylux.sh" "$fixture/bundle/install-sylux.sh"
chmod 0644 "$fixture/bundle/install-sylux.sh"
for asset in compose.yaml docker-beta.md env.example sylux-image.tar.gz; do
  printf 'fixture\n' > "$fixture/bundle/$asset"
done
printf '%s\n' "$image_id" > "$fixture/bundle/image-id.txt"
printf '{"version":"%s","source_commit":"%s","source_url":"https://github.com/nicoleman0/sylux/tree/%s","image_id":"%s","platform":"linux/amd64"}\n' \
  "$version" "$revision" "$revision" "$image_id" > "$fixture/bundle/release.json"
(cd "$fixture/bundle" && sha256sum -- * > SHA256SUMS)
cp "$fixture/bundle/install-sylux.sh" "$fixture/installer.sh"

cat > "$fixture/run.sh" <<'DRIVER'
#!/usr/bin/env bash
fixture=$1 scenario=$2 image_id=$3 revision=$4
shift 4
source "$fixture/installer.sh"
install_dir="$fixture/opt/sylux"
data_dir="$fixture/srv/sylux"
check_host() {
  [[ "$scenario" != unsupported ]] || fail 'Requires Ubuntu 24.04 x86_64.'
}
findmnt() {
  [[ "$*" == "-n -o FSTYPE -T $fixture/srv" || "$*" == "-n -o FSTYPE -T $data_dir" ]] || return 1
  if [[ "$scenario" == filesystem && "$5" == "$data_dir" ]]; then
    printf 'tmpfs\n'
  else
    printf 'ext4\n'
  fi
  [[ "$scenario" != storage-error ]]
}
find() {
  [[ "$scenario" != unreadable ]] || return 2
  command find "$@"
}
mv() {
  [[ "$scenario" != rename-error ]] || return 1
  command mv "$@"
}
download() { cp -- "$fixture/bundle/$3" "$2/$3"; }
make_tls() { mkdir -m 0700 "$1"; }
check_existing_docker() { :; }
docker() { fail 'The fixture must use only mocked Docker.'; }
ss() { :; }
system_identity() { [[ "$scenario" == install ]]; }
plan_packages() { packages=() tools=(); }
install_packages() { [[ $# == 0 ]]; }
docker_local() {
  local image
  case "$*" in
    'version --format {{.Server.Os}}/{{.Server.Arch}}')
      printf 'linux/amd64\n' ;;
    'info --format {{.OperatingSystem}}')
      printf 'Ubuntu 24.04\n' ;;
    'info --format {{json .SecurityOptions}}')
      printf '[]\n' ;;
    'compose version') ;;
    'ps --all --no-trunc --quiet --filter label=com.docker.compose.project=sylux')
      [[ "$scenario" != query-error ]] || return 2
      if [[ "$scenario" == stopped || "$scenario" == one-off || -f "$fixture/serving" ]]; then
        printf '%064d\n' 1
      fi
      ;;
    "load -i $install_dir/sylux-image.tar.gz")
      [[ "$scenario" == install ]] ;;
    'image inspect --format '*)
      image=${!#}
      [[ "$image" == sylux:v0.1.0 || "$image" == "$image_id" ]] || return 1
      printf '%s linux/amd64 v0.1.0 %s\n' "$image_id" "$revision" ;;
    'inspect --format {{.Image}} '*)
      printf '%s 10001:10001 host %s %s/compose.yaml sylux False\n' "$image_id" "$install_dir" "$install_dir" ;;
    'inspect --format {{.State.Running}} '*)
      printf 'true\n' ;;
    *) fail 'Unexpected Docker fixture command.' ;;
  esac
}
compose() {
  [[ "$scenario" == install ]] || fail 'Recovery attempted to change Docker state.'
  local record
  case "$*" in
    'config --quiet')
      for record in "$install_dir"/{version,phase}; do
        [[ -f "$record" && ! -L "$record" && "$(stat -c '%u:%g:%a:%h' "$record")" == 0:0:600:1 ]] ||
          fail "Expected a private installer record before init: ${record##*/}."
      done
      [[ -s "$install_dir/.env" ]] ;;
    'run --rm --no-deps -T sylux init --root /var/lib/sylux')
      [[ "$(<"$install_dir/phase")" == initializing ]] || fail 'Expected the initializing phase before Compose init.'
      [[ ! -f "$fixture/interrupt" ]] || return 1
      printf 'initialized\n' > "$data_dir/installation.json"
      mkdir "$data_dir/secrets"
      printf '%064d' 0 > "$data_dir/secrets/setup.authorization"
      ;;
    'up -d --no-recreate sylux') touch "$fixture/serving" ;;
    *) fail 'Unexpected Compose fixture command.' ;;
  esac
}
local_https() {
  case "$1" in
    readyz) printf '204\n' ;;
    setup) cat >/dev/null; printf '200\n' ;;
    *) fail 'Unexpected HTTPS fixture request.' ;;
  esac
}
main "$@"
DRIVER
chmod 0644 "$fixture/run.sh" "$fixture/installer.sh"

run() {
  bash "$fixture/run.sh" "$fixture" "$1" "$image_id" "$revision" "${@:2}"
}
snapshot() {
  python3 - "$fixture" "${1:-}" <<'PY'
import hashlib
import stat
import sys
from pathlib import Path

root = Path(sys.argv[1])
for parent in ("opt", "srv"):
    paths = sorted((root / parent).rglob("*"))
    for path in [root / parent, *paths]:
        name = str(path.relative_to(root))
        if name == sys.argv[2]:
            continue
        info = path.lstat()
        contents = None
        if stat.S_ISLNK(info.st_mode):
            contents = str(path.readlink())
        elif stat.S_ISREG(info.st_mode):
            contents = hashlib.sha256(path.read_bytes()).hexdigest()
        print(name, info.st_mode, info.st_uid, info.st_gid, info.st_nlink, info.st_ino, contents)
PY
}
refuse() {
  local expected=$1 before output
  shift
  before=$(snapshot)
  if output=$("$@" 2>&1) || [[ "$output" != *"$expected"* ]]; then
    printf 'Expected refusal %s, got %s\n' "$expected" "$output" >&2
    exit 1
  fi
  [[ "$(snapshot)" == "$before" ]] || { echo "Refusal changed the fixture: $expected" >&2; exit 1; }
  printf 'Refused without mutation: %s\n' "$expected"
}
reset=(ordinary --reset-empty-init)
phase="$install_dir/phase"
touch "$fixture/interrupt"
refuse 'Use --reset-empty-init on its own.' run "${reset[@]}" --yes
if output=$(run install "$version" --yes 2>&1) || [[ "$output" != *'Initialization failed.'* ]]; then
  printf 'Expected interrupted init, got %s\n' "$output" >&2
  exit 1
fi
rm "$fixture/interrupt"
cmp -- "$fixture/bundle/install-sylux.sh" "$install_dir/install-sylux.sh"
[[ "$(<"$phase")" == initializing ]]
refuse 'Initialization was interrupted' run install "$version" --yes
refuse 'Requires Ubuntu 24.04 x86_64.' run unsupported --reset-empty-init
refuse 'Run with sudo' runuser -u nobody -- bash "$fixture/run.sh" "$fixture" ordinary "$image_id" "$revision" --reset-empty-init

(
  flock -n 9
  refuse 'Another installer is running.' run "${reset[@]}"
) 9<"$fixture/opt"
chmod 0770 "$fixture/srv"
refuse 'must be root-owned and writable only by root' run "${reset[@]}"
chmod 0700 "$fixture/srv"
chmod 0750 "$install_dir"
refuse 'Expected a private installer directory' run "${reset[@]}"
chmod 0700 "$install_dir"
chmod 0750 "$data_dir"
refuse 'Unexpected /srv/sylux ownership or permissions.' run "${reset[@]}"
chmod 0700 "$data_dir"
chown 0:0 "$data_dir"
refuse 'Unexpected /srv/sylux ownership or permissions.' run "${reset[@]}"
chown 10001:10001 "$data_dir"
mv "$data_dir" "$data_dir.real"
ln -s "$data_dir.real" "$data_dir"
refuse 'Unexpected /srv/sylux ownership or permissions.' run "${reset[@]}"
rm "$data_dir"
mv "$data_dir.real" "$data_dir"
refuse 'Requires local ext4 storage' run filesystem --reset-empty-init
refuse 'Could not identify the filesystem' run storage-error --reset-empty-init

mv "$phase" "$phase.saved"
ln -s "$phase.saved" "$phase"
refuse 'Expected private, root-owned installer records' run "${reset[@]}"
rm "$phase"
mv "$phase.saved" "$phase"
chmod 0660 "$phase"
refuse 'Expected private, root-owned installer records' run "${reset[@]}"
chmod 0600 "$phase"
ln "$phase" "$phase.link"
refuse 'Expected private, root-owned installer records' run "${reset[@]}"
rm "$phase.link"
printf 'invalid\n' > "$install_dir/version"
refuse 'The recorded release tag is invalid.' run "${reset[@]}"
printf '%s\n' "$version" > "$install_dir/version"
printf 'initializing\n\n' > "$phase"
refuse 'Only an interrupted initializing phase can be reset.' run "${reset[@]}"
printf 'initializing\n' > "$phase"

touch "$data_dir/"$'\n'
refuse 'Data directory is not empty.' run "${reset[@]}"
rm "$data_dir/"$'\n'
refuse 'Could not check whether the data directory is empty.' run unreadable --reset-empty-init
refuse 'Sylux project containers exist.' run stopped --reset-empty-init
refuse 'Sylux project containers exist.' run one-off --reset-empty-init
refuse 'Could not list the local Sylux project containers.' run query-error --reset-empty-init
refuse 'Could not replace the initialization phase.' run rename-error --reset-empty-init

# Leave any staging entry from the interrupted install untouched.
ln -s "$install_dir/version" "$install_dir/phase.new"
before=$(snapshot opt/sylux/phase)
output=$(run "${reset[@]}")
[[ "$output" == "Empty initialization reset. Run the installer again with $version." ]]
[[ "$(<"$phase")" == prepared && "$(stat -c '%u:%g:%a:%h' "$phase")" == 0:0:600:1 ]]
[[ "$(snapshot opt/sylux/phase)" == "$before" ]]
refuse 'Only an interrupted initializing phase can be reset.' run "${reset[@]}"

output=$(run install "$version" --yes)
[[ "$output" == *'Sylux is ready'* && -f "$fixture/serving" ]]
digest=$(sha256sum "$data_dir/installation.json")
[[ "$(<"$phase")" == "${digest%% *}" ]]
[[ "$(cat "$install_dir/version")" == "$version" && -L "$install_dir/phase.new" ]]
refuse 'Only an interrupted initializing phase can be reset.' run "${reset[@]}"
before=$(snapshot)
output=$(run install "$version" --yes)
[[ "$output" == *'Sylux is ready'* && "$(snapshot)" == "$before" ]]
printf 'Empty reset changed only the phase. Normal install and same-release retry succeeded.\n'
