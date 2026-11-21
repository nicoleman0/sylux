#!/usr/bin/env bash
set +x
set -Eeuo pipefail

install_dir=/opt/sylux
data_dir=/srv/sylux
origin=https://127.0.0.1:3000
release_assets=(compose.yaml docker-beta.md env.example image-id.txt release.json sylux-image.tar.gz)

fail() {
  printf 'Sylux install: %s\n' "$*" >&2
  exit 1
}

check_host() {
  [[ "$(uname -s)/$(uname -m)" == Linux/x86_64 ]] ||
    fail 'Requires Ubuntu 24.04 x86_64.'
  # shellcheck source=/dev/null
  . /etc/os-release
  [[ "$ID/$VERSION_ID" == ubuntu/24.04 ]] || fail 'Requires Ubuntu 24.04 x86_64.'
  [[ "$(ps -p 1 -o comm=)" == systemd ]] || fail 'Requires a systemd host.'
  if systemd-detect-virt --container --quiet; then
    fail 'Run on the Ubuntu host, outside containers.'
  fi
}

download() {
  curl -q --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --retry 3 --connect-timeout 15 --max-time 300 --output "$2/$3" \
    "https://github.com/nicoleman0/sylux/releases/download/$1/$3"
}

verify_bundle() {
  python3 - "$@" "${release_assets[@]}" <<'PY'
import hashlib
import json
import re
import sys
from pathlib import Path

root, version, *required = sys.argv[1:]
root = Path(root)
try:
    manifest = root.joinpath("SHA256SUMS").read_text(encoding="ascii")
    entries = {}
    for line in manifest.splitlines():
        digest, name = re.fullmatch(r"([a-f0-9]{64})  ([a-z0-9.-]+)", line).groups()
        if name in entries:
            raise ValueError()
        entries[name] = digest
    if set(entries) not in (set(required), set(required) | {"install-sylux.sh"}):
        raise ValueError()
    for name, digest in entries.items():
        with root.joinpath(name).open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != digest:
                raise ValueError()
    metadata = json.loads(root.joinpath("release.json").read_text())
    build_id = root.joinpath("image-id.txt").read_text().removesuffix("\n")
    revision = metadata["source_commit"]
    if (
        metadata["version"] != version
        or metadata["platform"] != "linux/amd64"
        or not re.fullmatch(r"[a-f0-9]{40}", revision)
        or metadata["source_url"] != f"https://github.com/nicoleman0/sylux/tree/{revision}"
        or not re.fullmatch(r"sha256:[a-f0-9]{64}", build_id)
        or metadata["image_id"] != build_id
    ):
        raise ValueError()
except (OSError, ValueError, AttributeError, KeyError, TypeError):
    sys.exit("Sylux install: release checksum or metadata validation failed.")
print(revision)
PY
}

fetch_bundle() {
  local version=$1 dir=$2 asset
  for asset in SHA256SUMS "${release_assets[@]}"; do
    download "$version" "$dir" "$asset" || fail "Could not download $asset for $version."
  done
  if grep -q '  install-sylux\.sh$' "$dir/SHA256SUMS"; then
    download "$version" "$dir" install-sylux.sh || fail "Could not download installer for $version."
  fi
  verify_bundle "$dir" "$version"
}

prepare_install() (
  local version=$1 stage revision
  stage=$(mktemp -d /opt/.sylux.XXXXXXXX)
  trap 'rm -rf -- "$stage"' EXIT
  revision=$(fetch_bundle "$version" "$stage")
  make_tls "$stage/tls"
  printf '%s\n' "$version" > "$stage/version"
  printf '%s\n' prepared > "$stage/phase"
  mv -T --no-clobber -- "$stage" "$install_dir"
  [[ ! -d "$stage" ]] || fail 'The install directory appeared during preparation.'
  trap - EXIT
  printf '%s\n' "$revision"
)

docker_local() {
  env -i PATH="$PATH" HOME=/root docker --host unix:///var/run/docker.sock "$@"
}

compose() {
  docker_local compose --project-name sylux --project-directory "$install_dir" \
    --env-file "$install_dir/.env" --file "$install_dir/compose.yaml" "$@"
}

check_existing_docker() {
  local snap_root=${1:-/snap} socket=${2:-/var/run/docker.sock}
  if [[ -e $snap_root/docker/current || -L $snap_root/docker/current || -e $snap_root/bin/docker || -L $snap_root/bin/docker ]] ||
     { ! command -v docker >/dev/null && [[ -e $socket || -L $socket ]]; }; then
    fail 'Found snap Docker or an ambiguous Docker socket. Resolve it before installation.'
  fi
}

plan_packages() {
  packages=()
  tools=()
  local utility
  for utility in curl openssl python3; do
    command -v "$utility" >/dev/null || tools+=("$utility")
  done
  packages=("${tools[@]}")
  if ! command -v docker >/dev/null; then
    packages+=(docker.io docker-compose-v2)
  fi
}

install_packages() {
  if (( $# )); then
    (umask 022; apt-get update; NEEDRESTART_MODE=l apt-get install --yes --no-remove --no-install-recommends ca-certificates "$@")
  fi
}

check_docker() {
  local server platform security
  platform=$(docker_local version --format '{{.Server.Os}}/{{.Server.Arch}}') ||
    fail 'Start a local Docker Engine on /var/run/docker.sock and retry.'
  [[ "$platform" == linux/amd64 ]] || fail 'Requires a local linux/amd64 Docker Engine.'
  server=$(docker_local info --format '{{.OperatingSystem}}') || fail 'Cannot inspect Docker Engine.'
  security=$(docker_local info --format '{{json .SecurityOptions}}') || fail 'Cannot inspect Docker security options.'
  [[ "$server" != *'Docker Desktop'* && "$security" != *rootless* && "$security" != *userns* ]] ||
    fail 'Requires a system Docker Engine without user namespace remapping.'
  docker_local compose version >/dev/null ||
    fail 'Install the Compose v2 plugin for your existing Docker Engine and retry.'
}

check_storage() {
  [[ "$(findmnt -n -o FSTYPE -T "$1")" == ext4 ]] ||
    fail 'Requires local ext4 storage for /srv/sylux.'
}

check_system_dir() {
  local ownership mode
  [[ -d "$1" && ! -L "$1" ]] || fail "Expected a real system directory at $1."
  ownership=$(stat -c '%u:%g' "$1")
  mode=$(stat -c '%a' "$1")
  if [[ "$ownership" != 0:0 ]] || (( (8#$mode & 022) != 0 )); then
    fail "$1 must be root-owned and writable only by root."
  fi
}

present() {
  [[ -e "$1" || -L "$1" ]]
}

private_dir() {
  [[ -d "$1" && ! -L "$1" && "$(stat -c '%u:%g:%a' "$1")" == "$2:700" ]]
}

system_identity() {
  local user group
  user=$(getent passwd sylux || :)
  group=$(getent group sylux || :)
  if [[ -z "$group" ]]; then
    ! getent group 10001 >/dev/null || fail 'GID 10001 is already in use.'
    groupadd --system --gid 10001 sylux
  else
    [[ "$group" == 'sylux:x:10001:' ]] || fail 'Unexpected sylux group.'
  fi
  if [[ -z "$user" ]]; then
    ! getent passwd 10001 >/dev/null || fail 'UID 10001 is already in use.'
    useradd --system --uid 10001 --gid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin sylux
  else
    [[ "$user" == sylux:x:10001:10001:*:/nonexistent:/usr/sbin/nologin ]] ||
      fail 'Unexpected sylux user.'
  fi
  [[ "$(getent passwd 10001)" == "$(getent passwd sylux)" &&
     "$(getent group 10001)" == "$(getent group sylux)" ]] ||
    fail 'Conflicting service UID or GID.'
}

make_tls() {
  local dir=$1
  mkdir -m 0700 "$dir"
  openssl req -x509 -newkey rsa:3072 -noenc -sha256 -days 365 -subj '/CN=Sylux local CA' \
    -addext 'basicConstraints=critical,CA:TRUE,pathlen:0' -addext 'keyUsage=critical,keyCertSign' \
    -keyout "$dir/ca.key" -out "$dir/ca.crt" 2>/dev/null || fail 'Could not generate the local CA.'
  openssl req -new -newkey rsa:3072 -noenc -subj '/CN=127.0.0.1' \
    -keyout "$dir/key.pem" -out "$dir/leaf.csr" 2>/dev/null || fail 'Could not generate the TLS key.'
  openssl x509 -req -sha256 -days 365 -in "$dir/leaf.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" \
    -out "$dir/cert.pem" -extfile /dev/stdin 2>/dev/null <<'EXT' || fail 'Could not sign the loopback certificate.'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=IP:127.0.0.1
EXT
  rm -- "$dir/ca.key" "$dir/leaf.csr"
  chown -R 10001:10001 "$dir"
  chmod 0600 "$dir/key.pem"
}

require_free_port() {
  local listeners
  listeners=$(ss -H -ltn '( sport = :3000 )') || fail 'Could not check port 3000.'
  [[ -z "$listeners" ]] || fail 'Port 3000 is already in use. Resolve the conflict and retry.'
}

project_containers() {
  docker_local ps --all --no-trunc --quiet --filter label=com.docker.compose.project=sylux
}

confirm_plan() {
  local answer
  cat <<'PLAN'
Sylux uses one local Docker Compose project and serves HTTPS on 127.0.0.1:3000.
It reserves the sylux service UID/GID 10001, stores the
verified release and generated TLS material in /opt/sylux, and initializes the
private /srv/sylux data directory. It loads the versioned image and starts Sylux.
Browser trust is a separate step you control.
PLAN
  if command -v docker >/dev/null; then
    printf 'It uses the existing compatible Engine and Compose as installed.\n'
  else
    printf 'It will install Docker Engine and Compose and start the Docker daemon.\n'
  fi
  if (( ${#packages[@]} )); then
    printf 'It will run apt-get update and install ca-certificates and: %s\n' "${packages[*]}"
  else
    printf 'No packages will be installed.\n'
  fi
  if [[ "${1:-}" != --yes ]]; then
    [[ -t 0 ]] || fail 'Review the plan and pass --yes to approve unattended installation.'
    read -r -p 'Apply this plan? [y/N] ' answer
    [[ "$answer" == y || "$answer" == Y ]] || fail 'Plan declined.'
  fi
}

inspect_image() {
  local image=$1 version=$2 revision=$3 inspected
  inspected=$(docker_local image inspect --format '{{.Id}} {{.Os}}/{{.Architecture}} {{index .Config.Labels "org.opencontainers.image.version"}} {{index .Config.Labels "org.opencontainers.image.revision"}}' "$image") ||
    fail 'The verified local image is unavailable.'
  local id=${inspected%% *}
  [[ "$id" =~ ^sha256:[a-f0-9]{64}$ && "$inspected" == "$id linux/amd64 $version $revision" ]] ||
    fail 'Loaded image platform, version or revision does not match the release.'
  printf '%s\n' "$id"
}

check_container() {
  local container=$1 image=$2 details
  details=$(docker_local inspect --format '{{.Image}} {{.Config.User}} {{.HostConfig.NetworkMode}} {{index .Config.Labels "com.docker.compose.project.working_dir"}} {{index .Config.Labels "com.docker.compose.project.config_files"}} {{index .Config.Labels "com.docker.compose.service"}} {{index .Config.Labels "com.docker.compose.oneoff"}}' "$container")
  [[ "$details" == "$image 10001:10001 host $install_dir $install_dir/compose.yaml sylux False" ]] ||
    fail 'Unexpected container in the sylux project. Inspect it before retrying.'
}

local_https() {
  local path=$1
  shift
  curl -q --silent --noproxy '*' --max-time 2 --cacert "$install_dir/tls/ca.crt" \
    --output /dev/null --write-out '%{http_code}' "$@" "$origin/$path"
}

setup_status() {
  {
    printf 'X-Setup-Authorization: ' &&
      cat "$data_dir/secrets/setup.authorization" &&
      printf '\n'
  } | local_https setup --header @-
}

start_service() {
  local container=$1 image=$2 running status
  if [[ -n "$container" ]]; then
    check_container "$container" "$image"
    running=$(docker_local inspect --format '{{.State.Running}}' "$container")
    if [[ "$running" != true ]]; then
      require_free_port
      compose start sylux
    fi
  else
    require_free_port
    compose up -d --no-recreate sylux
    container=$(project_containers)
    [[ "$container" =~ ^[a-f0-9]{64}$ ]] || fail 'Could not identify the Sylux service container.'
    check_container "$container" "$image"
  fi
  for _ in {1..30}; do
    running=$(docker_local inspect --format '{{.State.Running}}' "$container")
    if [[ "$running" == true ]] && status=$(local_https readyz) && [[ "$status" == 204 ]]; then
      printf 'Sylux is ready at %s\n' "$origin"
      printf 'From another computer, run: ssh -L 3000:127.0.0.1:3000 <host>\n'
      printf 'Export the public CA with: sudo cat %s/tls/ca.crt > sylux-loopback.crt\n' "$install_dir"
      printf 'Trust that certificate on the browser device before opening Sylux.\n'
      status=$(setup_status) || fail 'Could not check setup state.'
      case "$status" in
        200) printf 'Choose First-time setup. Read the one-use authorization in a private terminal with:\n  sudo -u sylux cat %s/secrets/setup.authorization\n' "$data_dir" ;;
        404) printf 'Sign in with your existing administrator credentials.\n' ;;
        *) fail 'Could not check setup state.' ;;
      esac
      return
    fi
    sleep 1
  done
  fail 'Readiness failed. Inspect the Sylux service locally and retain the data directory.'
}

main() {
  local version=${1:-} container revision phase image recorded init_digest
  if [[ "$version" == --help && $# -eq 1 ]]; then
    printf 'Usage: sudo bash install-sylux.sh vX.Y.Z-beta.N [--yes]\n'
    return
  fi
  [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-beta\.[1-9][0-9]*$ &&
     $# -le 2 && ( $# -eq 1 || "$2" == --yes ) ]] || fail 'Pass a published beta tag and optionally --yes.'
  export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
  umask 077
  check_host
  shopt -s inherit_errexit
  (( EUID == 0 )) || fail 'Run with sudo after reviewing the installer.'
  check_system_dir /opt
  check_system_dir /srv
  check_storage /srv
  exec 9</opt
  flock -n 9 || fail 'Another installer is running.'
  if present "$install_dir"; then
    private_dir "$install_dir" 0:0 || fail 'Unexpected /opt/sylux. Inspect it before retrying.'
    [[ "$(cat "$install_dir/version")" == "$version" ]] ||
      fail 'The install record does not match this release. Use the original version or the manual upgrade guide.'
    phase=$(cat "$install_dir/phase")
    [[ "$phase" == prepared || "$phase" =~ ^[a-f0-9]{64}$ ]] ||
      fail 'Initialization was interrupted or the install record is unknown. Preserve the data and inspect it before retrying.'
  else
    ! present "$data_dir" || fail 'Existing /srv/sylux has no installer record. Preserve it and use the Docker guide.'
    if getent passwd sylux >/dev/null || getent passwd 10001 >/dev/null ||
       getent group sylux >/dev/null || getent group 10001 >/dev/null; then
      fail 'The Sylux service name, UID or GID is already in use.'
    fi
    phase=prepared
  fi
  if present "$data_dir"; then
    private_dir "$data_dir" 10001:10001 || fail 'Unexpected /srv/sylux ownership or permissions.'
    check_storage "$data_dir"
  fi
  if [[ "$phase" == prepared ]]; then
    require_free_port
    if present "$data_dir"; then
      [[ -z "$(find "$data_dir" -mindepth 1 -maxdepth 1 -printf x -quit)" ]] ||
        fail 'Data directory is not empty. Preserve it and inspect it before retrying.'
    fi
  else
    [[ -f "$data_dir/installation.json" && ! -L "$data_dir/installation.json" ]] ||
      fail 'The recorded data directory or its installation manifest is missing.'
    init_digest=$(sha256sum "$data_dir/installation.json")
    [[ "${init_digest%% *}" == "$phase" ]] || fail 'The data directory does not match the installation record.'
  fi
  check_existing_docker
  plan_packages
  confirm_plan "${2:-}"
  if command -v docker >/dev/null; then
    check_docker
    container=$(project_containers)
    if [[ -n "$container" ]]; then
      [[ "$phase" != prepared && "$container" =~ ^[a-f0-9]{64}$ ]] ||
        fail 'Unexpected containers in the sylux project. Inspect them before retrying.'
    fi
  fi
  install_packages "${tools[@]}"
  if ! present "$install_dir"; then
    revision=$(prepare_install "$version")
  else
    revision=$(verify_bundle "$install_dir" "$version")
  fi
  if ! command -v docker >/dev/null; then
    install_packages docker.io docker-compose-v2
  fi
  check_docker
  container=$(project_containers)
  if [[ -n "$container" ]]; then
    [[ "$phase" != prepared && "$container" =~ ^[a-f0-9]{64}$ ]] ||
      fail 'Unexpected containers in the sylux project. Inspect them before retrying.'
  fi
  if [[ "$phase" == prepared ]]; then
    system_identity
    if ! present "$data_dir"; then
      install -d -o 10001 -g 10001 -m 0700 "$data_dir"
    fi
    check_storage "$data_dir"
    docker_local load -i "$install_dir/sylux-image.tar.gz"
    image=$(inspect_image "sylux:$version" "$version" "$revision")
    printf 'SYLUX_IMAGE=%s\nSYLUX_DATA_DIR=%s\nSYLUX_TLS_DIR=%s/tls\n' \
      "$image" "$data_dir" "$install_dir" > "$install_dir/.env"
    compose config --quiet
    require_free_port
    printf '%s\n' initializing > "$install_dir/phase.new"
    mv -T -- "$install_dir/phase.new" "$install_dir/phase"
    compose run --rm --no-deps -T sylux init --root /var/lib/sylux ||
      fail 'Initialization failed. Preserve the data and inspect it before retrying.'
    init_digest=$(sha256sum "$data_dir/installation.json")
    printf '%s\n' "${init_digest%% *}" > "$install_dir/phase.new"
    mv -T -- "$install_dir/phase.new" "$install_dir/phase"
  else
    IFS= read -r recorded < "$install_dir/.env"
    image=${recorded#SYLUX_IMAGE=}
    if [[ ! "$image" =~ ^sha256:[a-f0-9]{64}$ ]] ||
       ! cmp -s "$install_dir/.env" <(printf 'SYLUX_IMAGE=%s\nSYLUX_DATA_DIR=%s\nSYLUX_TLS_DIR=%s/tls\n' "$image" "$data_dir" "$install_dir"); then
      fail 'Compose settings differ from the installer record.'
    fi
    [[ "$(inspect_image "$image" "$version" "$revision")" == "$image" ]] || fail 'Unexpected local image.'
  fi
  start_service "$container" "$image"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
