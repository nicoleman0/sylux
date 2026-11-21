#!/usr/bin/env bash
set -Eeuo pipefail
# This removes the preinstalled Docker packages only on the disposable hosted VM.
[[ "${GITHUB_ACTIONS:-}/${RUNNER_ENVIRONMENT:-}/${RUNNER_OS:-}/${RUNNER_ARCH:-}" == true/github-hosted/Linux/X64 ]] ||
  { echo 'Requires a disposable GitHub-hosted Linux x64 runner.' >&2; exit 1; }
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
version=${SYLUX_QUALIFY_VERSION:-v0.1.0-beta.9}
mode=${SYLUX_QUALIFY_DOCKER:-clean}
installer=${SYLUX_QUALIFY_INSTALLER:-scripts/install-sylux.sh}
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

# The candidate script is sourced unchanged; only its network transport is replaced.
if [[ -n "$bundle" ]]; then
  bundle=$(cd "$bundle" && pwd)
  installer=$(cd "$(dirname "$installer")" && pwd)/$(basename "$installer")
  cat > "$fixture/candidate.sh" <<'WRAPPER'
#!/usr/bin/env bash
source "$1"
bundle=$2
shift 2
download() { cp -- "$bundle/$3" "$2/$3"; }
main "$@"
WRAPPER
  chmod 0755 "$fixture" "$fixture/candidate.sh"
  export SYLUX_QUALIFY_WRAPPER="$fixture/candidate.sh" SYLUX_QUALIFY_INSTALLER="$installer" SYLUX_QUALIFY_BUNDLE="$bundle"
fi

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
install_command=(bash "$installer" "$version" --yes)
if [[ -n "$bundle" ]]; then
  install_command=(bash "$fixture/candidate.sh" "$installer" "$bundle" "$version" --yes)
fi
if conflict=$(sudo "${install_command[@]}" 2>&1); then
  fail 'The port conflict was accepted.'
fi
[[ "$conflict" == *'Port 3000 is already in use.'* ]]
kill -0 "$listener"
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
[[ "$unknown" == *'Existing /srv/sylux has no installer record.'* ]]
[[ "$(sudo sha256sum /srv/sylux/marker)" == "$before" ]]
sudo test ! -e /opt/sylux
sudo rm -rf -- /srv/sylux

# Exercise the documented reset on known disposable state before Docker removal.
python3 - "$fixture/reset.sh" <<'PY_RESET'
from pathlib import Path
import sys
text = Path('docs/docker-beta.md').read_text()
section = text.split('### Recover an empty interrupted initialization', 1)[1]
Path(sys.argv[1]).write_text(section.split('```bash\n', 1)[1].split('\n```', 1)[0] + '\n')
PY_RESET
sudo mkdir -m 0700 /opt/sylux /srv/sylux
sudo chown 10001:10001 /srv/sylux
printf 'initializing\n' | sudo tee /opt/sylux/phase >/dev/null
printf 'fixture\n' | sudo tee /srv/sylux/marker >/dev/null
if sudo bash "$fixture/reset.sh"; then fail 'Reset accepted nonempty data.'; fi
[[ "$(sudo cat /opt/sylux/phase)" == initializing ]]
sudo test -f /srv/sylux/marker
sudo rm /srv/sylux/marker
sudo bash "$fixture/reset.sh"
[[ "$(sudo cat /opt/sylux/phase)" == prepared ]]
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
if (( ${#packages[@]} )); then
  sudo apt-get purge --yes "${packages[@]}"
fi
# The Ubuntu runner also installs Compose outside APT.
sudo rm -f -- /usr/libexec/docker/cli-plugins/docker-compose
sudo bash -c '! command -v docker >/dev/null'
elif [[ "$mode" == existing ]]; then
  dpkg-query -W docker-ce docker-ce-cli docker-compose-plugin >/dev/null
  docker compose version >/dev/null
else
  fail 'Unknown Docker qualification mode.'
fi

SYLUX_QUALIFY_VERSION="$version" SYLUX_QUALIFY_DOCKER="$mode" node scripts/qualify-install-sylux.mjs "$fixture"
