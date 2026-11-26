#!/usr/bin/env bash
set -Eeuo pipefail
# This removes the preinstalled Docker packages only on the disposable hosted VM.
[[ "${GITHUB_ACTIONS:-}/${RUNNER_ENVIRONMENT:-}/${RUNNER_OS:-}/${RUNNER_ARCH:-}" == true/github-hosted/Linux/X64 ]] ||
  { echo 'Requires a disposable GitHub-hosted Linux x64 runner.' >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
version=${SYLUX_QUALIFY_VERSION:-v0.1.0-beta.9}
mode=${SYLUX_QUALIFY_DOCKER:-clean}
bundle=${SYLUX_QUALIFY_BUNDLE:-}
fixture=$(mktemp -d "${RUNNER_TEMP:?}/sylux-install.XXXXXXXX")
listener=''
cleanup() {
  if [[ -n "$listener" ]]; then
    kill "$listener" 2>/dev/null || true
  fi
  rm -rf -- "$fixture"
}
trap cleanup EXIT
unset DEBUG PWDEBUG
umask 077

# Verify the runner and the absence of an installation before changing packages.
# shellcheck source=scripts/release-bundle.sh
source scripts/release-bundle.sh
check_host
check_storage /srv
assert_no_install() {
  if sudo test -e /opt/sylux || sudo test -L /opt/sylux ||
     sudo test -e /srv/sylux || sudo test -L /srv/sylux; then
    fail 'Expected a fresh host with no Sylux install.'
  fi
}
assert_no_install
printf 'Runner image: %s\n' "${ImageVersion:?}"

# Hosted images make /opt writable by everyone; the installer requires a safe parent.
[[ -d /opt && ! -L /opt ]] || fail 'Expected a real fixture directory at /opt.'
opt_state=$(stat -c '%u:%g:%a' /opt)
printf 'Fixture /opt ownership and mode: %s\n' "$opt_state"
if [[ "$opt_state" == 0:0:777 ]]; then
  sudo chmod 0755 /opt
  echo 'Normalized disposable runner /opt to 0755.'
fi
check_system_dir /opt
check_system_dir /srv
for identity in 'passwd sylux' 'passwd 10001' 'group sylux' 'group 10001'; do
  read -r database key <<< "$identity"
  if getent "$database" "$key" >/dev/null; then
    fail "Fixture identity collision: $database $key."
  fi
done
for utility in python3 ss flock stat findmnt getent; do
  command -v "$utility" >/dev/null || fail "Fixture utility missing: $utility."
done

expect_refusal() {
  local output=$1 expected=$2 label=$3 known
  [[ "$output" == *"$expected"* ]] && return 0
  for known in 'must be root-owned and writable only by root' 'Expected a real system directory' \
    'Requires local ext4 storage' 'service name, UID or GID is already in use' \
    'Another installer is running' 'Could not check port 3000' \
    'Port 3000 is already in use' 'Existing /srv/sylux has no installer record' \
    'Unexpected /srv/sylux ownership or permissions' 'Data directory is not empty' \
    'Sylux project containers exist' 'Requires Ubuntu 24.04 x86_64' \
    'Requires a systemd host' 'Run on the Ubuntu host, outside containers' \
    'Run with sudo' 'Could not identify the filesystem for /srv/sylux' \
    'Expected a private installer directory at /opt/sylux' \
    'Expected private, root-owned installer records for version and phase' \
    'Could not read the recorded release tag' 'The recorded release tag is invalid' \
    'Only an interrupted initializing phase can be reset' \
    'Could not check whether the data directory is empty'; do
    if [[ "$output" == *"$known"* ]]; then
      fail "$label: unexpected installer refusal category: $known."
    fi
  done
  if [[ "$output" == *"unbound variable"* ]]; then
    fail "$label: installer encountered an unbound variable."
  fi
  fail "$label: unexpected installer output (withheld)."
}

# The candidate script is sourced unchanged; only its network transport is replaced.
if [[ -n "$bundle" ]]; then
  bundle=$(cd "$bundle" && pwd)
  manifest=$(verify_release_bundle "$bundle" "$version" "${GITHUB_SHA:?}" "${SYLUX_QUALIFY_MANIFEST:?}")
  printf -- '- Installer bundle %s, source %s, SHA256SUMS %s, build image %s\n' \
    "$version" "$GITHUB_SHA" "$manifest" "$(<"$bundle/image-id.txt")" | tee -a "${GITHUB_STEP_SUMMARY:?}"
  existing=$(docker_local image ls -q "sylux:$version")
  [[ -z "$existing" ]] || fail 'The candidate tag is already present before the installer runs.'
  cat > "$fixture/candidate.sh" <<'WRAPPER'
#!/usr/bin/env bash
bundle=$1
shift
source "$bundle/install-sylux.sh"
download() { cp -- "$bundle/$3" "$2/$3"; }
main "$@"
WRAPPER
  chmod 0755 "$fixture" "$fixture/candidate.sh"
  export SYLUX_QUALIFY_WRAPPER="$fixture/candidate.sh" SYLUX_QUALIFY_BUNDLE="$bundle"
fi

# A fresh installation must leave a pre-existing loopback listener alone.
python3 - "$fixture/listening" <<'PY' &
import socket, sys, time
from pathlib import Path
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 3000))
    sock.listen()
    Path(sys.argv[1]).touch()
    time.sleep(60)
PY
listener=$!
for _ in {1..50}; do
  [[ ! -f "$fixture/listening" ]] || break
  sleep 0.1
done
[[ -f "$fixture/listening" ]] || fail 'Port listener did not become ready.'
install_command=(bash scripts/install-sylux.sh "$version" --yes)
if [[ -n "$bundle" ]]; then
  install_command=(bash "$fixture/candidate.sh" "$bundle" "$version" --yes)
fi
if conflict=$(sudo "${install_command[@]}" 2>&1); then
  fail 'The port conflict was accepted.'
fi
expect_refusal "$conflict" 'Port 3000 is already in use.' 'Port conflict fixture'
kill -0 "$listener" || fail 'Port listener exited before the refusal was checked.'
assert_no_install
kill "$listener"
wait "$listener" || true
listener=''
echo 'Port conflict was refused before host preparation.'

# Existing unknown data must remain byte-for-byte untouched.
sudo mkdir -m 0700 /srv/sylux
printf 'unknown data\n' | sudo tee /srv/sylux/marker >/dev/null
before=$(sudo sha256sum /srv/sylux/marker)
if unknown=$(sudo "${install_command[@]}" 2>&1); then fail 'Unknown data was accepted.'; fi
expect_refusal "$unknown" 'Existing /srv/sylux has no installer record.' 'Unknown data fixture'
[[ "$(sudo sha256sum /srv/sylux/marker)" == "$before" ]] || fail 'Unknown data marker changed.'
sudo test ! -e /opt/sylux || fail 'Unknown data fixture created an install record.'
sudo rm -rf -- /srv/sylux

# Exercise the installed recovery command before Docker removal.
sudo mkdir -m 0700 /opt/sylux /srv/sylux
sudo chown 10001:10001 /srv/sylux
sudo install -m 0600 "${bundle:-scripts}/install-sylux.sh" /opt/sylux/install-sylux.sh
sudo bash -c 'umask 077; printf "%s\n" "$1" >/opt/sylux/version; printf "initializing\n" >/opt/sylux/phase' _ "$version"
reset_command=(sudo bash /opt/sylux/install-sylux.sh --reset-empty-init)
printf 'fixture\n' | sudo tee /srv/sylux/marker >/dev/null
if nonempty=$("${reset_command[@]}" 2>&1); then fail 'Reset accepted nonempty data.'; fi
expect_refusal "$nonempty" 'Data directory is not empty.' 'Reset with data fixture'
[[ "$(sudo cat /opt/sylux/phase)" == initializing ]] || fail 'Reset changed the initializing phase with data present.'
sudo test -f /srv/sylux/marker || fail 'Reset removed the data marker.'
sudo rm /srv/sylux/marker
sudo docker pull alpine:3.20 >/dev/null
reset_container=$(sudo docker create --label com.docker.compose.project=sylux \
  --label com.docker.compose.oneoff=True alpine:3.20 true)
if occupied=$("${reset_command[@]}" 2>&1); then fail 'Reset accepted an existing project container.'; fi
expect_refusal "$occupied" 'Sylux project containers exist.' 'Reset with one-off container fixture'
[[ "$(sudo cat /opt/sylux/phase)" == initializing ]] || fail 'Reset changed the initializing phase with a container present.'
sudo docker rm "$reset_container" >/dev/null
"${reset_command[@]}"
[[ "$(sudo cat /opt/sylux/phase)" == prepared ]] || fail 'Reset did not produce the prepared phase.'
printf 'fixture\n' | sudo tee /srv/sylux/marker >/dev/null
if prepared=$(sudo "${install_command[@]}" 2>&1); then fail 'Prepared state accepted nonempty data.'; fi
expect_refusal "$prepared" 'Data directory is not empty.' 'Prepared data fixture'
[[ "$(sudo cat /opt/sylux/phase)" == prepared ]] || fail 'Prepared data refusal changed the phase.'
sudo test -f /srv/sylux/marker || fail 'Prepared data refusal removed the marker.'
sudo rm /srv/sylux/marker /opt/sylux/version /opt/sylux/install-sylux.sh
sudo rm /opt/sylux/phase
sudo rmdir /opt/sylux /srv/sylux
assert_no_install

if [[ "$mode" == clean ]]; then
# Remove only installed Docker-family packages. Leave the VM and its ext4 intact.
packages=()
for package in docker-ce docker-ce-cli docker-ce-rootless-extras docker-buildx-plugin docker-compose-plugin containerd.io docker.io docker-compose-v2 podman-docker; do
  if [[ "$(dpkg-query -W -f='${db:Status-Status}' "$package" 2>/dev/null)" == installed ]]; then
    packages+=("$package")
  fi
done
sudo systemctl stop docker.service docker.socket containerd.service
if (( ${#packages[@]} )); then
  sudo apt-get purge --yes "${packages[@]}"
fi
# The Ubuntu runner also installs Compose outside APT.
sudo rm -f -- /usr/libexec/docker/cli-plugins/docker-compose /run/docker.sock
for socket in /run/docker.sock /var/run/docker.sock; do
  if sudo test -e "$socket" || sudo test -L "$socket"; then
    fail "Stale Docker socket remains at $socket."
  fi
done
sudo bash -c '! command -v docker >/dev/null'
elif [[ "$mode" == existing ]]; then
  for package in containerd.io docker-ce docker-ce-cli; do
    [[ "$(dpkg-query -W -f='${db:Status-Status}' "$package")" == installed ]] || fail "Required package $package is not installed."
  done
  docker compose version >/dev/null
else
  fail 'Unknown Docker qualification mode.'
fi

SYLUX_QUALIFY_VERSION="$version" SYLUX_QUALIFY_DOCKER="$mode" node scripts/qualify-install-sylux.mjs "$fixture"
qualify_docker() { sudo -n env -i PATH="$PATH" HOME=/root docker --host unix:///var/run/docker.sock "$@"; }
engine_version=$(qualify_docker version --format '{{.Server.Version}}') || fail 'Cannot read Docker Engine version.'
compose_version=$(qualify_docker compose version --short) || fail 'Cannot read Docker Compose version.'
safe_version='^[A-Za-z0-9.+~-]+$'
[[ "$engine_version" =~ $safe_version ]] || fail 'Docker Engine returned an empty or unsafe version.'
[[ "$compose_version" =~ $safe_version ]] || fail 'Docker Compose returned an empty or unsafe version.'
printf -- '- Ubuntu 24.04 x86_64, runner %s, local ext4, Docker Engine %s, Compose %s (%s)\n' \
  "$ImageVersion" "$engine_version" "$compose_version" "$mode" | tee -a "${GITHUB_STEP_SUMMARY:?}"
if [[ -n "$bundle" ]]; then
  sudo cmp -- "$bundle/install-sylux.sh" /opt/sylux/install-sylux.sh
  image=$(inspect_image "sylux:$version" "$version" "$GITHUB_SHA")
  container=$(qualify_docker compose --project-directory /opt/sylux ps -q sylux)
  [[ "$(docker_local inspect --format '{{.Image}}' "$container")" == "$image" ]]
  {
    printf -- '- Locally loaded and running %s, linux/amd64, OCI version %s, revision %s\n' "$image" "$version" "$GITHUB_SHA"
    printf -- '- Trusted Chromium passed administrator creation, fresh sign-in, first upload, byte-exact download, restart and same-version rerun.\n'
  } | tee -a "$GITHUB_STEP_SUMMARY"
fi
