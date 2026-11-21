#!/usr/bin/env bash
set -Eeuo pipefail
asset_dir=$(cd -- "${1:?Pass the downloaded beta.9 bundle directory}" && pwd)
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=scripts/install-sylux.sh
source "$root/scripts/install-sylux.sh"

expected=9fd9d7495493859f113d1a22ef060dc736dd0b9e
version=v0.1.0-beta.9
[[ "$(verify_bundle "$asset_dir" "$version")" == "$expected" ]]

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
cp "$asset_dir/"* "$fixture/"
original=$asset_dir/SHA256SUMS
reject_bundle() {
  local message
  if message=$(verify_bundle "$fixture" "$1" 2>&1); then
    fail 'Accepted an invalid release bundle.'
  fi
  [[ "$message" == *'release checksum or metadata validation failed'* ]]
}

reject_bundle v0.1.0-beta.8
rm -- "$fixture/env.example"
reject_bundle "$version"
cp "$asset_dir/env.example" "$fixture/"
printf '\n' >> "$fixture/compose.yaml"
reject_bundle "$version"
cp "$asset_dir/compose.yaml" "$fixture/"
head -n 1 "$original" >> "$fixture/SHA256SUMS"
reject_bundle "$version"
awk 'NR == 1 { $0 = $1 "  ../compose.yaml" } { print }' "$original" > "$fixture/SHA256SUMS"
reject_bundle "$version"

# Exercise the future release shape with the checked-out installer.
cp "$root/scripts/install-sylux.sh" "$fixture/"
cp "$original" "$fixture/SHA256SUMS"
(cd "$fixture" && sha256sum install-sylux.sh >> SHA256SUMS)
[[ "$(verify_bundle "$fixture" "$version")" == "$expected" ]]
printf '\n' >> "$fixture/install-sylux.sh"
reject_bundle "$version"
printf 'Published assets, checksum and metadata refusals, and installer asset check passed.\n'

data_dir=$fixture
mkdir "$data_dir/secrets"
printf '%064d' 0 > "$data_dir/secrets/setup.authorization"
local_https() {
  [[ "$*" == 'setup --header @-' ]]
  local header
  IFS= read -r header
  [[ "$header" == "X-Setup-Authorization: $(printf '%064d' 0)" ]]
  printf '200\n'
}
[[ "$(setup_status)" == 200 ]]
printf 'The setup-status request sends the authorization through stdin.\n'

uname() { printf 'unsupported\n'; }
if message=$(check_host 2>&1); then
  fail 'Accepted an unsupported host.'
fi
[[ "$message" == *'Requires Ubuntu 24.04 x86_64.'* ]]
printf 'Unsupported-host guard passed.\n'

# No host mutation: exercise snap and socket discovery against private fixtures.
mkdir -p "$fixture/snap/docker"
mkdir "$fixture/snap/docker/current"
if message=$(check_existing_docker "$fixture/snap" "$fixture/socket" 2>&1); then
  fail 'Accepted snap Docker.'
fi
[[ "$message" == *'ambiguous Docker socket'* ]]
rmdir "$fixture/snap/docker/current"
docker() { :; }
check_existing_docker "$fixture/snap" "$fixture/socket"
printf 'Snap refusal and healthy Docker detection passed.\n'

(
  # Used indirectly by check_existing_docker.
  # shellcheck disable=SC2329
  command() { if [[ "$*" == '-v docker' ]]; then return 1; fi; builtin command "$@"; }
  touch "$fixture/socket"
  if message=$(check_existing_docker "$fixture/snap" "$fixture/socket" 2>&1); then
    fail 'Accepted a socket without a Docker client.'
  fi
  [[ "$message" == *'ambiguous Docker socket'* ]]
)
(
  apt-get() { [[ "$(umask)" == 0022 ]]; }
  umask 077
  install_packages fixture-package
)
printf 'Ambiguous socket and package umask guards passed.\n'
