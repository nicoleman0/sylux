#!/usr/bin/env bash
set -Eeuo pipefail
# This removes the preinstalled Docker packages only on the disposable hosted VM.
[[ "${GITHUB_ACTIONS:-}/${RUNNER_ENVIRONMENT:-}/${RUNNER_OS:-}/${RUNNER_ARCH:-}" == true/github-hosted/Linux/X64 ]] ||
  { echo 'Requires a disposable GitHub-hosted Linux x64 runner.' >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
version=v0.1.0-beta.9
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
# shellcheck source=scripts/install-sylux.sh
source scripts/install-sylux.sh
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

# A beta cannot own a pre-existing process on the loopback port.
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
[[ -f "$fixture/listening" ]]
if conflict=$(sudo bash scripts/install-sylux.sh "$version" --yes 2>&1); then
  fail 'The port conflict was accepted.'
fi
[[ "$conflict" == *'Port 3000 is already in use.'* ]]
kill -0 "$listener"
assert_no_install
kill "$listener"
wait "$listener" || true
listener=''
echo 'Port conflict was refused before host preparation.'

# Remove only installed Docker-family packages. Leave the VM and its ext4 intact.
packages=()
for package in docker-ce docker-ce-cli docker-ce-rootless-extras docker-buildx-plugin docker-compose-plugin containerd.io docker.io docker-compose-v2 podman-docker; do
  if [[ "$(dpkg-query -W -f='${db:Status-Status}' "$package" 2>/dev/null)" == installed ]]; then
    packages+=("$package")
  fi
done
if (( ${#packages[@]} )); then
  sudo apt-get purge --yes "${packages[@]}"
fi
# The Ubuntu runner also installs Compose outside APT.
sudo rm -f -- /usr/libexec/docker/cli-plugins/docker-compose
sudo bash -c '! command -v docker >/dev/null'

node scripts/qualify-install-sylux.mjs "$fixture"
