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

marker=sylux-00000000000000000000000000000000
config=$(printf 'description: %s\narch: amd64\nostype: debian\nunprivileged: 1\nrootfs: local:volume\n' "$marker")
stock_ct_config "$marker" <<< "$config"
reject stock_ct_config "${marker%0}1" <<< "$config"
reject stock_ct_config "$marker" <<< "${config/unprivileged: 1/unprivileged: 0}"
for setting in 'features: nesting=1' 'mp0: local:other' 'dev0: /dev/fuse' 'lxc.apparmor.profile: unconfined' 'hookscript: local:script'; do
  reject stock_ct_config "$marker" <<< "$config"$'\n'"$setting"
done
echo 'Native checksums, exact identity, tamper rejection and CT configuration guards passed.'

docker build --platform linux/amd64 --quiet -t sylux-native-fixture:debian13 - <<'DOCKERFILE'
FROM debian:trixie-slim@sha256:ab3b3c8c5159eebfca4376430132d53c4d425f869c765ecbe47177a030da575d
RUN apt-get update && apt-get install --yes --no-install-recommends libgcc-s1 openssl python3 systemd && rm -rf /var/lib/apt/lists/*
DOCKERFILE
docker run --rm -i --platform linux/amd64 --network none --cap-drop SYS_ADMIN \
  --mount "type=bind,src=$test_bundle,dst=/bundle,readonly" \
  --mount "type=bind,src=$PWD/scripts/rehearse-docker-upgrade.py,dst=/fixture-http.py,readonly" \
  sylux-native-fixture:debian13 bash -s -- "$test_version" "$test_revision" "$manifest" <<'CONTAINER'
set -Eeuo pipefail
umask 077
version=$1 revision=$2 manifest=$3
fixture=$(mktemp -d)
export GITHUB_STEP_SUMMARY=/dev/null
bash /bundle/install-sylux-lxc.sh verify /bundle "$version" "$revision" "$manifest"
if bash /bundle/install-sylux-lxc.sh install /bundle "$version" "$revision" "$manifest" > "$fixture/reject.log" 2>&1; then
  echo 'Guest installer incorrectly accepted a Docker container.' >&2
  exit 1
fi
grep -Fq 'Requires a running systemd LXC guest.' "$fixture/reject.log"
test ! -e /opt/sylux && test ! -e /srv/sylux && test ! -e /etc/sylux
install -d -m 0755 /opt/sylux
tar -xzf "/bundle/sylux-$version-linux-amd64.tar.gz" -C /opt/sylux
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
runuser -u sylux -- /opt/sylux/sylux init --root /srv/sylux
# Match the service arguments and identity without requiring systemd or host privileges.
start() {
  setpriv --reuid=sylux --regid=sylux --init-groups /opt/sylux/sylux serve --root /srv/sylux \
    --listen 127.0.0.1:3000 --tls-cert /etc/sylux/cert.pem --tls-key /etc/sylux/key.pem \
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
