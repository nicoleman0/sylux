#!/usr/bin/env bash
set -Eeuo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
[[ $# == 4 ]] || { echo 'Usage: scripts/test-native.sh BUNDLE VERSION SOURCE_SHA MANIFEST_SHA256' >&2; exit 1; }
test_bundle=$(cd -- "$1" && pwd) test_version=$2 test_revision=$3 manifest=$4
# shellcheck source=deploy/lxc/ct/sylux.sh
source deploy/lxc/ct/sylux.sh

verify() { bash "$lxc_dir/install/sylux-install.sh" verify "$@"; }
reject() {
  if "$@" > /dev/null 2>&1; then
    echo "Unexpected successful test: $1" >&2
    exit 1
  fi
}

verify "$test_bundle" "$test_version" "$test_revision" "$manifest"
other=ffffffffffffffffffffffffffffffffffffffff
[[ "$test_revision" != "$other" ]]
reject verify "$test_bundle" "$test_version" "$other" "$manifest"
reject verify "$test_bundle" 'latest' "$test_revision" "$manifest"
reject verify "$test_bundle" "$test_version" "$test_revision" "$(printf '%064d' 0)"

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
cp -R -- "$test_bundle" "$fixture/bundle"
touch "$fixture/bundle/extra"
reject verify "$fixture/bundle" "$test_version" "$test_revision" "$manifest"
rm -- "$fixture/bundle/extra"
printf '\n' >> "$fixture/bundle/install-sylux-lxc.sh"
reject verify "$fixture/bundle" "$test_version" "$test_revision" "$manifest"
cp -- "$test_bundle/install-sylux-lxc.sh" "$fixture/bundle/"
archive="sylux-$test_version-linux-amd64.tar.gz"
printf 'invalid archive with a matching checksum\n' | gzip -n > "$fixture/bundle/$archive"
(
  cd "$fixture/bundle"
  sha256sum -- install-sylux-lxc.sh "$archive" > SHA256SUMS-native
)
invalid_manifest=$(sha256sum "$fixture/bundle/SHA256SUMS-native")
reject verify "$fixture/bundle" "$test_version" "$test_revision" "${invalid_manifest%% *}"

echo 'Native checksums, exact identity and tamper rejection passed.'

docker build --platform linux/amd64 --quiet -t sylux-native-fixture:debian13 - <<'DOCKERFILE'
FROM debian:trixie-slim@sha256:ab3b3c8c5159eebfca4376430132d53c4d425f869c765ecbe47177a030da575d
RUN apt-get update && apt-get install --yes --no-install-recommends libgcc-s1 openssl python3 systemd && rm -rf /var/lib/apt/lists/*
DOCKERFILE
docker run --rm -i --platform linux/amd64 --network none --cap-drop SYS_ADMIN \
  --mount "type=bind,src=$test_bundle,dst=/bundle,readonly" \
  --mount "type=bind,src=$PWD/deploy/lxc,dst=/lxc,readonly" \
  --mount "type=bind,src=$PWD/scripts/qualify-native-lxc.sh,dst=/qualify.sh,readonly" \
  --mount "type=bind,src=$PWD/scripts/rehearse-docker-upgrade.py,dst=/fixture-http.py,readonly" \
  sylux-native-fixture:debian13 bash -s -- "$test_version" "$test_revision" "$manifest" <<'CONTAINER'
set -Eeuo pipefail
umask 077
version=$1 revision=$2 manifest=$3
fixture=$(mktemp -d)
export GITHUB_STEP_SUMMARY=/dev/null
unit_start=$(sed -n 's/^ExecStart=//p' /bundle/install-sylux-lxc.sh)

reject_with() {
  local expected=$1
  shift
  if ( "$@" ) > "$fixture/guard.log" 2>&1 || ! grep -Fq "$expected" "$fixture/guard.log"; then
    cat "$fixture/guard.log" >&2
    echo "Expected rejection: $expected" >&2
    exit 1
  fi
}

(
  # The host helper requires root, so run its pct-stubbed checks here.
  source /lxc/ct/sylux.sh
  marker=sylux-0123456789abcdef0123456789abcdef
  stock=$(printf 'description: %s%%0A\narch: amd64\nostype: debian\nunprivileged: 1\nrootfs: local:volume\n' "$marker")
  ct_current=$stock ct_pending=$stock
  mismatch='CT marker or stock unprivileged Debian configuration did not match.'
  pct() {
    case "$*" in
      'config 123 --current 1') printf '%s\n' "$ct_current" ;;
      'config 123') printf '%s\n' "$ct_pending" ;;
      'status 123') echo 'status: running' ;;
      'exec 123 -- mktemp -d /root/sylux-stage.XXXXXXXX')
        if [[ "$stage_result" == error ]]; then return 1; fi
        printf '%s\n' "$stage_result" ;;
      'exec 123 -- timeout 90 systemctl is-system-running --wait') printf '%s\n' "$boot_status"; return "$boot_exit" ;;
      'exec 123 -- systemctl --failed --no-legend --no-pager --plain') printf '%s\n' "$failed_units"; return "$units_exit" ;;
      *) printf '%s\n' "$*" >> "$fixture/pct-unexpected"; return 89 ;;
    esac
  }
  check_ct 123 "$marker"
  ct_current=${stock/\%0A/} ct_pending=${stock/\%0A/}
  check_ct 123 "$marker"
  ct_current="$stock"$'\nfeatures: nesting=1' ct_pending="$stock"$'\nfeatures: nesting=1'
  check_ct 123 "$marker"
  ct_pending="$stock"
  reject_with "$mismatch" check_ct 123 "$marker"
  for addition in "description: $marker%0A" 'description: conflicting' \
    'unprivileged: 1' 'unprivileged: 0' 'lock: backup' \
    $'features: nesting=1\nfeatures: nesting=1' 'features: nesting=1,keyctl=1' \
    'features: fuse=1' 'features: nesting=0' 'features: mknod=1' \
    'mp0: local:other' 'dev0: /dev/fuse' 'lxc.apparmor.profile: unconfined' \
    'lxc.mount.auto: proc:rw' 'hookscript: local:script'; do
    ct_current="$stock"$'\n'"$addition" ct_pending="$stock"$'\n'"$addition"
    reject_with "$mismatch" check_ct 123 "$marker"
  done
  for ct_current in "${stock/\%0A/%0A%0A}" "${stock/"$marker"/"${marker%f}0"}" \
    "${stock/unprivileged: 1/unprivileged: 0}"; do
    ct_pending=$ct_current
    reject_with "$mismatch" check_ct 123 "$marker"
  done
  ct_current=$stock ct_pending=$stock
  stage_result=error
  reject_with 'Could not create a private guest staging directory.' \
    lxc_install install 123 "$marker" /bundle "$version" "$revision" "$manifest"
  stage_result=/root/foreign
  reject_with 'Unexpected guest staging path.' \
    lxc_install install 123 "$marker" /bundle "$version" "$revision" "$manifest"

  source <(sed -n '/^check_offline_ct() {$/,/^check_offline_ct$/p' /qualify.sh | sed '$d')
  ct=123
  check_offline_ct
  ct_current="$stock"$'\nnet0: name=eth0,bridge=vmbr0,ip=dhcp' ct_pending="$stock"$'\nnet0: name=eth0,bridge=vmbr0,ip=dhcp'
  reject_with 'Qualification requires a NIC-less CT.' check_offline_ct
  boot_status=running boot_exit=0 failed_units='' units_exit=0
  guest_boot_health
  boot_status=degraded boot_exit=1 failed_units='networking.service loaded failed failed'
  reject_with 'Guest did not settle into a healthy systemd boot.' guest_boot_health
  boot_status=starting boot_exit=124 failed_units=''
  reject_with 'Guest did not settle into a healthy systemd boot.' guest_boot_health
  units_exit=1
  reject_with 'Could not inspect failed guest units.' guest_boot_health
  test ! -e "$fixture/pct-unexpected"
)

(
  # Exercise the production guard with systemd 257 property values.
  source <(sed '/^command=/,$d' /bundle/install-sylux-lxc.sh)
  unit=/etc/systemd/system/sylux.service
  drop_in='' fragment=$unit reload=no state=inactive
  args='/opt/sylux/current/sylux serve --root ${SYLUX_DATA_ROOT} --listen 127.0.0.1:3000 --tls-cert /etc/sylux/cert.pem --tls-key /etc/sylux/key.pem'
  exec_start="{ path=/opt/sylux/current/sylux ; argv[]=$args ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }"
  systemctl() {
    case "$*" in
      'show sylux.service -p DropInPaths --value') echo "$drop_in" ;;
      'show sylux.service -p FragmentPath --value') echo "$fragment" ;;
      'show sylux.service -p NeedDaemonReload --value') echo "$reload" ;;
      'show sylux.service -p ExecStart --value') echo "$exec_start" ;;
      'show sylux.service -p ActiveState --value') echo "$state" ;;
      *) return 89 ;;
    esac
  }
  check_upgrade_unit
  ( drop_in=/etc/systemd/system/sylux.service.d/restore.conf; reject_with 'Remove or reconcile service drop-ins' check_upgrade_unit )
  ( fragment=/run/systemd/system/sylux.service; reject_with 'Reconcile the service unit override' check_upgrade_unit )
  ( reload=yes; reject_with 'Reload systemd and reconcile' check_upgrade_unit )
  ( exec_start=${exec_start//current/releases\/v0.1.0-beta.10}; reject_with 'The effective service command is unfamiliar' check_upgrade_unit )
  ( exec_start="$exec_start $exec_start"; reject_with 'The effective service command is unfamiliar' check_upgrade_unit )
  ( state=failed; reject_with 'systemctl reset-failed sylux.service' check_upgrade_unit )
  ( systemctl() { return 1; }; reject_with 'Could not inspect sylux.service (DropInPaths).' check_upgrade_unit )
  [[ "$unit_start" == "$args" ]]
  [[ "$(sed -n 's/^Restart=//p' /bundle/install-sylux-lxc.sh)" == on-failure ]]
)
echo 'CT ownership, configuration, boot, staging and upgrade guard regressions passed.'

bash /bundle/install-sylux-lxc.sh verify /bundle "$version" "$revision" "$manifest"
if bash /bundle/install-sylux-lxc.sh install /bundle "$version" "$revision" "$manifest" > "$fixture/reject.log" 2>&1; then
  echo 'Guest installer incorrectly accepted a Docker container.' >&2
  exit 1
fi
grep -Fq 'Requires a running systemd LXC guest.' "$fixture/reject.log"
test ! -e /opt/sylux && test ! -e /srv/sylux && test ! -e /etc/sylux
install -d -m 0755 /opt/sylux
install -d -m 0755 "/opt/sylux/releases/$version"
tar -xzf "/bundle/sylux-$version-linux-amd64.tar.gz" -C "/opt/sylux/releases/$version"
ln -s "releases/$version" /opt/sylux/current
useradd --system --user-group --no-create-home --shell /usr/sbin/nologin sylux
install -d -o sylux -g sylux -m 0700 /srv/sylux
install -d -o root -g sylux -m 0750 /etc/sylux
openssl req -x509 -newkey rsa:3072 -sha256 -days 1 -noenc \
  -subj /CN=127.0.0.1 -addext subjectAltName=IP:127.0.0.1 \
  -keyout /etc/sylux/key.pem -out /etc/sylux/cert.pem 2>/dev/null
chgrp sylux /etc/sylux/key.pem
chmod 0640 /etc/sylux/key.pem
chmod 0644 /etc/sylux/cert.pem
cp /etc/sylux/cert.pem "$fixture/trust.pem"
runuser -u sylux -- /opt/sylux/current/sylux init --root /srv/sylux
read -r -a service_argv <<< "${unit_start/'${SYLUX_DATA_ROOT}'//srv/sylux}"
start() {
  setpriv --reuid=sylux --regid=sylux --init-groups "${service_argv[@]}" \
    > "$fixture/service.log" 2>&1 &
  pid=$!
  python3 /fixture-http.py ready "$fixture"
}
start
python3 /fixture-http.py seed "$fixture" < /srv/sylux/secrets/setup.authorization
kill -TERM "$pid"
wait "$pid"
start
python3 /fixture-http.py verify "$fixture"
uid=$(id -u sylux)
gid=$(id -g sylux)
test "$(stat -c '%u:%g:%a' /srv/sylux)" = "$uid:$gid:700"
test -z "$(find /srv/sylux -xdev \( ! -uid "$uid" -o ! -gid "$gid" -o -perm /077 \) -print -quit)"
kill -TERM "$pid"
wait "$pid"
echo 'Offline Debian 13 amd64 fixture passed exact-byte setup, upload and restart. Proxmox and systemd boot require the live CT test.'
CONTAINER
