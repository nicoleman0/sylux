#!/usr/bin/env bash
set +x
set -Eeuo pipefail

lxc_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
lxc_fail() { printf 'Sylux Proxmox: %s\n' "$*" >&2; exit 1; }

stock_ct_config() {
  awk -v marker="$1" '
    $0 == "description: " marker { owned++ }
    $0 == "unprivileged: 1" { unprivileged++ }
    $0 == "arch: amd64" { amd64++ }
    $0 == "ostype: debian" { debian++ }
    /^rootfs: / { rootfs++ }
    /^\[/ || /^(lxc[.]|mp[0-9]+:|dev[0-9]+:|hookscript:|features:|lock:)/ { exceptions++ }
    END { exit !(owned == 1 && unprivileged == 1 && amd64 == 1 && debian == 1 &&
                  rootfs == 1 && !exceptions) }
  '
}

check_ct() {
  local ct=$1 marker=$2 config pending
  [[ $EUID == 0 && "$ct" =~ ^[1-9][0-9]{2,8}$ && "$marker" =~ ^sylux-[a-f0-9]{32}$ ]] ||
    lxc_fail 'Run as PVE root with an explicit CT ID and a sylux-<32 lowercase hex> ownership marker.'
  config=$(pct config "$ct" --current 1) || lxc_fail 'The selected local CT is unavailable.'
  pending=$(pct config "$ct") || lxc_fail 'Could not inspect pending config.'
  if [[ "$config" != "$pending" ]] || ! stock_ct_config "$marker" <<< "$config"; then
    lxc_fail 'CT marker or stock unprivileged Debian configuration did not match.'
  fi
}

lxc_install() (
  local action=$1 ct=$2 marker=$3 bundle=$4 version=$5 revision=$6 digest=$7 stage file
  [[ "$action" =~ ^(install|upgrade)$ ]] || lxc_fail 'Expected install or upgrade.'
  check_ct "$ct" "$marker"
  bash "$lxc_dir/install/sylux-install.sh" verify "$bundle" "$version" "$revision" "$digest"
  [[ "$(pct status "$ct")" == "status: running" ]] || lxc_fail 'Start the intended CT first.'
  stage=$(pct exec "$ct" -- mktemp -d /root/sylux-stage.XXXXXXXX)
  [[ "$stage" =~ ^/root/sylux-stage\.[a-zA-Z0-9]{8}$ ]] || lxc_fail 'Unexpected guest staging path.'
  trap 'pct exec "$ct" -- rm -rf -- "$stage"' EXIT
  for file in SHA256SUMS-native install-sylux-lxc.sh "sylux-$version-linux-amd64.tar.gz"; do
    pct push "$ct" "$bundle/$file" "$stage/$file" --user 0 --group 0 --perms 0600
  done
  # Check the exact staged script and archive before running the installer as root.
  # shellcheck disable=SC2016
  pct exec "$ct" -- bash -c '
    set -euo pipefail
    cd "$1"
    printf "%s  SHA256SUMS-native\n" "$2" | sha256sum --check --status
    sha256sum --check --status SHA256SUMS-native
  ' bash "$stage" "$digest"
  pct exec "$ct" -- bash "$stage/install-sylux-lxc.sh" "$action" "$stage" "$version" "$revision" "$digest"
)

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  [[ $# == 7 ]] ||
    lxc_fail 'Usage: ct/sylux.sh install|upgrade CTID MARKER BUNDLE VERSION SOURCE_SHA MANIFEST_SHA256'
  lxc_install "$@"
fi
