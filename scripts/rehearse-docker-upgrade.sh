#!/usr/bin/env bash
set +x
set -Eeuo pipefail
umask 077

[[ "${GITHUB_ACTIONS:-}/${RUNNER_ENVIRONMENT:-}/${RUNNER_OS:-}/${RUNNER_ARCH:-}" == true/github-hosted/Linux/X64 ]] ||
  { echo 'Requires a disposable GitHub-hosted Linux x64 runner.' >&2; exit 1; }

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
# shellcheck source=scripts/install-sylux.sh
source scripts/install-sylux.sh
check_host
fixture="${RUNNER_TEMP:?}/sylux-upgrade"
project=sylux-upgrade-rehearsal
docker_local() { sudo -n env -i PATH="$PATH" HOME=/root docker --host unix:///var/run/docker.sock "$@"; }
prior=v0.1.0-beta.9
candidate=v0.1.0-beta.10

prior_manifest=144ebe0038ae63679b2c7b0a96a2f5c416bd631d0f88f3c572d99ff52ac43425

rehearsal_compose() {
  local release=$1 env_file=$2
  shift 2
  docker_local compose --project-name "$project" --project-directory "$fixture/$release" \
    --env-file "$env_file" --file "$fixture/$release/compose.yaml" "$@"
}

if [[ "${1:-}" == cleanup ]]; then
  if [[ -f "$fixture/owned" ]]; then
    cleanup_failed=0
    if containers=$(docker_local ps -aq --filter "label=com.docker.compose.project=$project" 2>/dev/null); then
      if [[ -n "$containers" ]]; then
        mapfile -t ids <<< "$containers"
        docker_local rm -f "${ids[@]}" >/dev/null 2>&1 || cleanup_failed=1
      fi
    else
      cleanup_failed=1
    fi
    if [[ -d "$fixture/archive" ]]; then
      if mountpoint -q "$fixture/archive"; then
        sudo -n umount "$fixture/archive" >/dev/null 2>&1 || cleanup_failed=1
      elif [[ $? != 32 ]]; then
        cleanup_failed=1
      fi
    fi
    (( cleanup_failed == 0 )) || fail 'Could not remove the disposable containers or unmount the archive.'
    sudo -n rm -rf -- "$fixture" >/dev/null 2>&1 || fail 'Could not remove the private rehearsal workspace.'
  fi
  exit
fi

phase=${1:-}
exec 3>&1
fail() {
  printf 'Docker rehearsal: %s\n' "$*" >&3
  exit 1
}

case "$phase" in
  upgrade)
    existing=$(docker_local ps -aq --filter "label=com.docker.compose.project=$project")
    [[ -z "$existing" ]] || fail 'The rehearsal Compose project already has containers.'
    mkdir -m 0700 -- "$fixture"
    touch "$fixture/owned"
    ;;
  rollback)
    [[ -f "$fixture/owned" && -f "$fixture/upgrade-complete" ]] ||
      fail 'Rollback requires a successful upgrade in this job.'
    ;;
  *) fail 'Usage: rehearse-docker-upgrade.sh upgrade|rollback|cleanup' ;;
esac

stage='opening the private log'
trap 'status=$?; trap - ERR; printf "%s rehearsal failed while %s (line %s, exit %s; output withheld).\n" "$phase" "$stage" "$LINENO" "$status" >&3' ERR
exec >>"$fixture/private.log" 2>&1
unset DEBUG PWDEBUG

evidence() {
  printf '%s\n' "$*" >&3
  printf -- '- %s\n' "$*" >>"${GITHUB_STEP_SUMMARY:?}"
}

prepare_release() {
  local release=$1 manifest_hash=$2 release_dir="$fixture/$1"
  local revision image_id archive_hash existing_image
  mkdir "$release_dir"
  revision=$(fetch_bundle "$release" "$release_dir")
  printf '%s  %s\n' "$manifest_hash" "$release_dir/SHA256SUMS" | sha256sum --check --status
  existing_image=$(docker_local image ls -q "sylux:$release")
  [[ -z "$existing_image" ]]
  docker_local load --input "$release_dir/sylux-image.tar.gz"
  image_id=$(inspect_image "sylux:$release" "$release" "$revision")
  printf '%s\n' "$image_id" >"$release_dir/local-image-id"
  if [[ "$release" == "$prior" ]]; then
    sed -e "s|^SYLUX_IMAGE=.*|SYLUX_IMAGE=$image_id|" \
      -e "s|^SYLUX_DATA_DIR=.*|SYLUX_DATA_DIR=$fixture/live|" \
      -e "s|^SYLUX_TLS_DIR=.*|SYLUX_TLS_DIR=$fixture/tls|" "$release_dir/env.example" >"$release_dir/.env"
  else
    sed "s|^SYLUX_IMAGE=.*|SYLUX_IMAGE=$image_id|" "$fixture/$prior/.env" >"$release_dir/.env"
  fi
  rehearsal_compose "$release" "$release_dir/.env" config -q
  archive_hash=$(sha256sum "$release_dir/sylux-image.tar.gz")
  evidence "Package: https://github.com/nicoleman0/sylux/releases/tag/$release"
  evidence "Source $revision, manifest $manifest_hash, image archive ${archive_hash%% *}"
  evidence "Build image $(<"$release_dir/image-id.txt"), loaded image $image_id"
}

start_release() {
  local release=$1 env_file=$2 root=$3 image_id container
  image_id=$(<"$fixture/$release/local-image-id")
  # Check the effective service before starting a host-networked container.
  rehearsal_compose "$release" "$env_file" config --format json |
    jq -e --arg image "$image_id" --arg root "$root" --arg tls "$fixture/tls" '
      (.services | keys) == ["sylux"] and
      (.services.sylux | .image == $image and .pull_policy == "never" and
        .network_mode == "host" and .user == "10001:10001" and
        .command == null and .entrypoint == null and
        (.ports // []) == [] and
        (.volumes | length == 2 and
          any(.[]; .type == "bind" and .source == $root and .target == "/var/lib/sylux") and
          any(.[]; .type == "bind" and .source == $tls and .target == "/run/sylux/tls" and .read_only == true)))' >/dev/null
  [[ "$(docker_local image inspect --format '{{json .Config.Cmd}}' "$image_id")" == \
    '["serve","--root","/var/lib/sylux","--listen","127.0.0.1:3000","--tls-cert","/run/sylux/tls/cert.pem","--tls-key","/run/sylux/tls/key.pem"]' ]]
  rehearsal_compose "$release" "$env_file" up -d --no-build --pull never
  container=$(rehearsal_compose "$release" "$env_file" ps --status running -q sylux)
  [[ -n "$container" && "$(docker_local inspect --format '{{.Image}}' "$container")" == "$image_id" ]]
  [[ "$(docker_local inspect --format '{{range .Mounts}}{{if eq .Destination "/var/lib/sylux"}}{{.Source}}{{end}}{{end}}' "$container")" == "$root" ]]
  python3 scripts/rehearse-docker-upgrade.py ready "$fixture"
  require_loopback_listener
  evidence "$release served with its loaded image over certificate-verified TLS on host loopback."
}

require_loopback_listener() {
  local listening
  listening=$(ss -H -ltn 'sport = :3000' | awk '{print $4}')
  [[ "$listening" == 127.0.0.1:3000 ]]
}

stop_release() {
  local release=$1 env_file="$fixture/$1/.env" container running listening
  container=$(rehearsal_compose "$release" "$env_file" ps --status running -q sylux)
  [[ -n "$container" && "$(docker_local inspect --format '{{.Image}}' "$container")" == \
    "$(<"$fixture/$release/local-image-id")" ]]
  rehearsal_compose "$release" "$env_file" stop
  [[ "$(docker_local inspect --format '{{.State.Status}} {{.State.ExitCode}}' "$container")" == "exited 0" ]]
  running=$(docker_local ps -q --filter "label=com.docker.compose.project=$project")
  [[ -z "$running" ]]
  listening=$(ss -H -ltn 'sport = :3000')
  [[ -z "$listening" ]]
}

verify_pre_upgrade_archive() {
  local expected actual
  mountpoint -q "$fixture/archive"
  expected=$(<"$fixture/pre-upgrade-archive.sha256")
  [[ "$expected" =~ ^[a-f0-9]{64}$ ]]
  sudo sh -c 'cd "$1" && sha256sum --strict --status -c backup.tar.sha256' sh "$fixture/archive"
  actual=$(sudo sha256sum "$fixture/archive/backup.tar")
  [[ "${actual%% *}" == "$expected" ]]
}

rollback() {
  local prior_dir="$fixture/$prior" restored="$fixture/restored" revision image_id live_identity contents
  stage='confirming loopback restriction and stopping the upgraded service'
  require_loopback_listener
  stop_release "$candidate"
  evidence "$candidate exited cleanly. No rehearsal container is running and the loopback port is closed."

  stage='rechecking the pre-upgrade archive and matching published release'
  verify_pre_upgrade_archive
  printf '%s  %s\n' "$prior_manifest" "$prior_dir/SHA256SUMS" | sha256sum --check --status
  revision=$(verify_bundle "$prior_dir" "$prior")
  image_id=$(<"$prior_dir/local-image-id")
  [[ "$(inspect_image "$image_id" "$prior" "$revision")" == "$image_id" ]]
  evidence "Rollback inputs: stopped archive SHA-256 $(<"$fixture/pre-upgrade-archive.sha256"), $prior source $revision, manifest $prior_manifest, loaded image $image_id."

  stage='creating and checking a fresh restore root'
  live_identity=$(stat -c '%d:%i' "$fixture/live")
  sudo mkdir -m 0700 -- "$restored"
  sudo chown 10001:10001 "$restored"
  private_dir "$restored" 10001:10001
  check_storage "$restored"
  [[ "$(stat -c %d "$restored")" == "$(stat -c %d "$fixture/live")" ]]
  [[ "$(stat -c '%d:%i' "$restored")" != "$live_identity" ]]
  contents=$(sudo find "$restored" -mindepth 1 -print -quit)
  [[ -z "$contents" ]]

  stage='extracting the verified archive into the fresh root'
  sudo tar --numeric-owner --same-owner --same-permissions \
    -C "$restored" -xpf "$fixture/archive/backup.tar"
  private_dir "$restored" 10001:10001
  evidence 'Extracted into a newly created empty root on the same local ext4 filesystem as the upgraded root.'
  sed "s|^SYLUX_DATA_DIR=.*|SYLUX_DATA_DIR=$restored|" "$prior_dir/.env" >"$prior_dir/rollback.env"

  stage='starting the prior published release against the restored root'
  start_release "$prior" "$prior_dir/rollback.env" "$restored"

  stage='comparing the rollback with the pre-upgrade baseline'
  python3 scripts/rehearse-docker-upgrade.py verify "$fixture" >&3 2>&3
  python3 scripts/rehearse-docker-upgrade.py trash-bytes "$fixture" >&3 2>&3
  [[ "$(stat -c '%d:%i' "$fixture/live")" == "$live_identity" ]]
  require_loopback_listener
  evidence "Fresh-root rollback to $prior passed. The upgraded root was retained and recipient access stayed restricted to host loopback."
}

if [[ "$phase" == rollback ]]; then
  rollback
  exit
fi

stage='preparing supported storage and the loopback certificate'
listening=$(ss -H -ltn 'sport = :3000')
[[ -z "$listening" ]]
[[ "$(docker_local info --format '{{.OSType}}/{{.Architecture}}')" == linux/x86_64 ]]
for database in passwd group; do
  if getent "$database" 10001 >/dev/null; then
    fail 'Fixture UID or GID is already in use.'
  fi
done

mkdir "$fixture/archive"
sudo install -d -o 10001 -g 10001 -m 0700 "$fixture/live" "$fixture/tls"
check_storage "$fixture/live"
live_device=$(findmnt -n -o SOURCE -T "$fixture/live")
live_major_minor=$(findmnt -rn -o MAJ:MIN -T "$fixture/live")
[[ "$live_major_minor" =~ ^[0-9]+:[0-9]+$ && -e "/sys/dev/block/$live_major_minor" ]]
truncate -s 128M "$fixture/archive.ext4"
mkfs.ext4 -q -F "$fixture/archive.ext4"
sudo mount -o loop,nosuid,nodev,noexec "$fixture/archive.ext4" "$fixture/archive"
sudo chmod 0700 "$fixture/archive"
check_storage "$fixture/archive"
archive_device=$(findmnt -n -o SOURCE --mountpoint "$fixture/archive")
archive_major_minor=$(findmnt -rn -o MAJ:MIN --mountpoint "$fixture/archive")
[[ -b "$archive_device" && "$live_major_minor" != "$archive_major_minor" ]]
[[ "$(stat -c %d "$fixture/live")" != "$(stat -c %d "$fixture/archive")" ]]
docker_version=$(docker_local version --format '{{.Server.Version}}')
compose_version=$(docker_local compose version --short)
[[ "$docker_version" =~ ^[a-zA-Z0-9.+~-]+$ && "$compose_version" =~ ^[a-zA-Z0-9.+~-]+$ ]]
kernel=$(uname -r)
image_os=${ImageOS:-unknown}
image_version=${ImageVersion:-unknown}
[[ "$kernel" =~ ^[a-zA-Z0-9.+~-]+$ && "$image_os" =~ ^[a-zA-Z0-9.+~-]+$ && "$image_version" =~ ^[a-zA-Z0-9.+~-]+$ ]]
evidence "Runner image $image_os $image_version, kernel $kernel"
evidence "Host: Ubuntu 24.04 x86_64, Docker $docker_version, Compose $compose_version"
evidence "Mounts: live ext4 ($live_device), distinct mounted archive ext4 ($archive_device)"

sudo openssl req -x509 -newkey rsa:3072 -sha256 -days 1 -noenc \
  -subj '/CN=127.0.0.1' -addext 'subjectAltName=IP:127.0.0.1' \
  -keyout "$fixture/tls/key.pem" -out "$fixture/tls/cert.pem"
sudo chown 10001:10001 "$fixture/tls/"{key,cert}.pem
sudo chmod 0600 "$fixture/tls/key.pem"
# Copy only the public certificate into the client trust file.
# shellcheck disable=SC2024
sudo cat "$fixture/tls/cert.pem" >"$fixture/trust.pem"
openssl verify -CAfile "$fixture/trust.pem" -verify_ip 127.0.0.1 "$fixture/trust.pem"

stage='verifying and loading the beta.9 release'
prepare_release "$prior" "$prior_manifest"
stage='initializing and starting beta.9'
rehearsal_compose "$prior" "$fixture/$prior/.env" run --rm -T --pull never sylux init --root /var/lib/sylux
start_release "$prior" "$fixture/$prior/.env" "$fixture/live"
stage='seeding and comparing the beta.9 fixture'
sudo cat "$fixture/live/secrets/setup.authorization" |
  python3 scripts/rehearse-docker-upgrade.py seed "$fixture" >&3 2>&3
python3 scripts/rehearse-docker-upgrade.py verify "$fixture" >&3 2>&3

stage='stopping beta.9 and verifying its pre-upgrade archive'
stop_release "$prior"
sudo tar --numeric-owner -C "$fixture/live" -cpf "$fixture/archive/backup.tar" .
sudo chmod 0600 "$fixture/archive/backup.tar"
sudo tar -tf "$fixture/archive/backup.tar" >/dev/null
sudo sh -c 'cd "$1" && umask 077 && sha256sum backup.tar > backup.tar.sha256 && sha256sum -c backup.tar.sha256' \
  sh "$fixture/archive"
archive_hash=$(sudo cut -d' ' -f1 "$fixture/archive/backup.tar.sha256")
printf '%s\n' "$archive_hash" >"$fixture/pre-upgrade-archive.sha256"
evidence "Verified stopped-service pre-upgrade archive SHA-256: $archive_hash"

stage='verifying and loading the beta.10 release'
prepare_release "$candidate" d6d61886c01d21e21f4bd1a9150423b30d8f88631c53d6e5a2376d133bbff349
stage='starting beta.10 on the existing root'
start_release "$candidate" "$fixture/$candidate/.env" "$fixture/live"
stage='comparing beta.10 with the beta.9 baseline'
python3 scripts/rehearse-docker-upgrade.py verify "$fixture" >&3 2>&3
stage='verifying the original bytes of retained Trash on beta.10'
python3 scripts/rehearse-docker-upgrade.py trash-bytes "$fixture" >&3 2>&3
stage='recording beta.10 named library grant and revocation'
python3 scripts/rehearse-docker-upgrade.py grants "$fixture" >&3 2>&3
python3 scripts/rehearse-docker-upgrade.py verify-grants "$fixture" >&3 2>&3
stage='checking retained rollback inputs'
verify_pre_upgrade_archive
[[ "$(inspect_image "sylux:$prior" "$prior" "$(jq -r .source_commit "$fixture/$prior/release.json")")" == \
  "$(<"$fixture/$prior/local-image-id")" ]]
evidence 'Verified archive, prior package/configuration and expected results retained for same-job reuse.'
touch "$fixture/upgrade-complete"
