#!/usr/bin/env bash
set +x
set -Eeuo pipefail
umask 077

if [[ ${1:-} == guest ]]; then
  # This entry point is installed only in the checked disposable guest.
  [[ $EUID == 0 && $# == 4 && "$2" =~ ^/root/sylux-rehearse\.[a-zA-Z0-9]{8}$ && -d "$2" && ! -L "$2" ]]
  fixture=$2 phase=$3 version=$4
  [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-beta\.[1-9][0-9]*)?$ ]]
  cd -- "$fixture"
  export GITHUB_STEP_SUMMARY=/dev/null
  live=/srv/sylux
  restored=/srv/sylux-restored
  account_uid=$(id -u sylux)
  account_gid=$(id -g sylux)

  stopped() {
    systemctl stop sylux.service
    [[ "$(systemctl show sylux.service -p ActiveState,Result,MainPID --value | sort)" == $'0\ninactive\nsuccess' ]]
    [[ -z "$(ss -H -ltn 'sport = :3000')" ]]
  }

  check_ownership() {
    local directory=$1 invalid
    [[ "$(stat -c '%u:%g:%a' "$directory")" == "$account_uid:$account_gid:700" ]]
    invalid=$(find "$directory" -xdev \( ! -uid "$account_uid" -o ! -gid "$account_gid" \
      -o -perm /077 -o \( ! -type d -a ! -type f \) \) -print -quit)
    [[ -z "$invalid" ]]
    [[ "$(stat -c '%u:%g:%a' /etc/sylux/key.pem)" == "0:$account_gid:640" ]]
    printf 'Canonical data ownership is %s:%s. The root is mode 0700 and TLS private key is separate.\n' \
      "$account_uid" "$account_gid"
  }

  ready() {
    local pid binary
    python3 fixture-http.py ready "$fixture"
    [[ "$(ss -H -ltn 'sport = :3000' | awk '{print $4}')" == 127.0.0.1:3000 ]]
    [[ "$(systemctl show sylux.service -p User,Group --value | sort -u)" == sylux ]]
    pid=$(systemctl show sylux.service -p MainPID --value)
    [[ "$pid" =~ ^[1-9][0-9]*$ ]]
    awk -v uid="$account_uid" -v gid="$account_gid" '
      $1 == "Uid:" && $2 == uid && $3 == uid && $4 == uid && $5 == uid { u = 1 }
      $1 == "Gid:" && $2 == gid && $3 == gid && $4 == gid && $5 == gid { g = 1 }
      END { exit !(u && g) }
    ' "/proc/$pid/status"
    binary=$(readlink -- "/proc/$pid/exe")
    [[ "$binary" == "/opt/sylux/releases/$version/sylux" ]]
    check_ownership "$1"
    printf 'Verified running %s, certificate trust and loopback-only listener.\n' "$version"
  }

  case "$phase" in
    seed)
      systemctl is-enabled --quiet sylux.service
      cp -- /etc/sylux/cert.pem trust.pem
      openssl verify -CAfile trust.pem -verify_ip 127.0.0.1 trust.pem
      ready "$live"
      python3 fixture-http.py seed "$fixture" < "$live/secrets/setup.authorization"
      python3 fixture-http.py verify "$fixture"
      stopped
      systemctl start sylux.service
      ready "$live"
      python3 fixture-http.py verify "$fixture"
      ;;
    backup)
      ready "$live"
      python3 fixture-http.py verify "$fixture"
      stopped
      check_ownership "$live"
      tar --create --numeric-owner --file pre-upgrade.tar --directory "$live" .
      sha256sum pre-upgrade.tar > pre-upgrade.tar.sha256
      cat pre-upgrade.tar.sha256
      ;;
    upgraded)
      systemctl start sylux.service
      ready "$live"
      python3 fixture-http.py verify "$fixture"
      python3 fixture-http.py trash-bytes "$fixture"
      stopped
      ;;
    restore)
      stopped
      sha256sum --check pre-upgrade.tar.sha256
      [[ ! -e "$restored" && ! -L "$restored" ]]
      install -d -o sylux -g sylux -m 0700 "$restored"
      [[ "$(stat -c '%d:%i' "$restored")" != "$(stat -c '%d:%i' "$live")" ]]
      tar --extract --numeric-owner --same-owner --same-permissions \
        --file pre-upgrade.tar --directory "$restored"
      tar --compare --numeric-owner --file pre-upgrade.tar --directory "$restored"
      check_ownership "$restored"
      install -d -m 0755 /etc/systemd/system/sylux.service.d
      cat > /etc/systemd/system/sylux.service.d/restore.conf <<UNIT
[Service]
Environment=SYLUX_DATA_ROOT=$restored
ExecStart=
ExecStart=/opt/sylux/releases/$version/sylux serve --root \${SYLUX_DATA_ROOT} --listen 127.0.0.1:3000 --tls-cert /etc/sylux/cert.pem --tls-key /etc/sylux/key.pem
UNIT
      systemctl daemon-reload
      systemctl start sylux.service
      ready "$restored"
      python3 fixture-http.py verify "$fixture"
      python3 fixture-http.py trash-bytes "$fixture"
      stopped
      printf 'Stopped archive compared intact before booting the restored root with %s.\n' "$version"
      ;;
    *) exit 1 ;;
  esac
  exit
fi

[[ $# == 10 ]] || {
  echo 'Usage: scripts/qualify-native-lxc.sh CTID MARKER PRIOR_BUNDLE PRIOR_VERSION PRIOR_SOURCE_SHA PRIOR_MANIFEST_SHA256 CANDIDATE_BUNDLE CANDIDATE_VERSION CANDIDATE_SOURCE_SHA CANDIDATE_MANIFEST_SHA256' >&2
  exit 1
}
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=deploy/lxc/ct/sylux.sh
source "$script_dir/../deploy/lxc/ct/sylux.sh"
ct=$1 marker=$2 prior_bundle=$3 prior=$4 prior_sha=$5 prior_digest=$6
candidate_bundle=$7 candidate=$8 candidate_sha=$9 candidate_digest=${10}
check_ct "$ct" "$marker"
bash "$lxc_dir/install/sylux-install.sh" verify "$prior_bundle" "$prior" "$prior_sha" "$prior_digest"
bash "$lxc_dir/install/sylux-install.sh" verify "$candidate_bundle" "$candidate" "$candidate_sha" "$candidate_digest"
[[ "$prior" != "$candidate" ]] || lxc_fail 'Select distinct prior and candidate tags.'
[[ "$(pct status "$ct")" == "status: running" ]] || lxc_fail 'Start the intended CT first.'
storage=$(pct config "$ct" --current 1 | awk -F ': ' '$1 == "rootfs" {split($2, volume, ":"); print volume[1]}')
[[ "$storage" =~ ^[a-zA-Z][a-zA-Z0-9_.-]*$ ]] || lxc_fail 'Expected a Proxmox-managed guest root volume.'
pvesm status --storage "$storage" | awk '$2 == "lvmthin" && $3 == "active" { local_disk = 1 }
  END { exit !local_disk }' || lxc_fail 'Qualification requires an active local LVM-thin root.'
[[ "$(pct exec "$ct" -- findmnt -n -o FSTYPE /)" == ext4 ]] || lxc_fail 'Qualification requires an ext4 guest root.'
printf 'Guest root is on active local LVM-thin storage and mounted as ext4.\n'
pct exec "$ct" -- findmnt -n -o SOURCE,FSTYPE /
pct exec "$ct" -- cat /proc/1/uid_map
fixture=$(pct exec "$ct" -- mktemp -d /root/sylux-rehearse.XXXXXXXX)
[[ "$fixture" =~ ^/root/sylux-rehearse\.[a-zA-Z0-9]{8}$ ]] || lxc_fail 'Unexpected guest fixture directory.'
phase=staging
report_failure() {
  printf 'LXC qualification failed during %s (exit %s). Retain the owned CT for inspection.\n' "$phase" "$1" >&2
}
trap 'report_failure "$?"' ERR
pct push "$ct" "$script_dir/qualify-native-lxc.sh" "$fixture/rehearse.sh" --user 0 --group 0 --perms 0600
pct push "$ct" "$script_dir/rehearse-docker-upgrade.py" "$fixture/fixture-http.py" --user 0 --group 0 --perms 0600
guest() { pct exec "$ct" -- bash "$fixture/rehearse.sh" guest "$fixture" "$@"; }

phase='prior install and first setup'
lxc_install install "$ct" "$marker" "$prior_bundle" "$prior" "$prior_sha" "$prior_digest"
guest seed "$prior"
phase='CT restart'
check_ct "$ct" "$marker"
pct shutdown "$ct" --timeout 60
[[ "$(pct status "$ct")" == "status: stopped" ]]
pct start "$ct"
phase='verification after CT restart and stopped backup'
guest backup "$prior"
phase='populated upgrade'
lxc_install upgrade "$ct" "$marker" "$candidate_bundle" "$candidate" "$candidate_sha" "$candidate_digest"
guest upgraded "$candidate"
phase='restore in the original CT'
guest restore "$prior"
check_ct "$ct" "$marker"
printf 'Native LXC qualification passed using one CT. The service is stopped.\n'
