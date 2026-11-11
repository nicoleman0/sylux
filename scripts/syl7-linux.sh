#!/usr/bin/env bash
set -euo pipefail

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || { echo 'Requires Linux x86_64' >&2; exit 1; }
base=$(mktemp -d "${RUNNER_TEMP:-/tmp}/sylux-recovery.XXXXXXXX")
name="sylux-recovery-$$"
image="${SYLUX_TEST_IMAGE:-$name:local}"
client_name="$name-ssh-client"
ssh_port=32222
cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker rm -f "$client_name" >/dev/null 2>&1 || true
  if [[ -z "${SYLUX_TEST_IMAGE:-}" ]]; then
    docker image rm "$image" >/dev/null 2>&1 || true
  fi
  sudo pkill -F "$base/ssh/sshd.pid" 2>/dev/null || true
  sudo rm -rf -- "$base"
}
trap cleanup EXIT
mkdir -m 700 "$base/root" "$base/restored" "$base/tls"
fs_type=$(findmnt -n -o FSTYPE -T "$base")
printf 'Kernel: '; uname -srmo
printf 'Host filesystem: %s\n' "$fs_type"
findmnt -T "$base" -o TARGET,SOURCE,FSTYPE,OPTIONS
[[ "$fs_type" == ext4 ]] || { echo 'Host filesystem is not ext4' >&2; exit 1; }
docker version
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$base/tls/key.pem" -out "$base/tls/cert.pem" -days 1 -subj '/CN=localhost' -addext 'subjectAltName=IP:127.0.0.1' >/dev/null 2>&1
chmod 644 "$base/tls/cert.pem"
chmod 600 "$base/tls/key.pem"
sudo chown 10001:10001 "$base/root" "$base/restored" "$base/tls"
sudo chown 10001:10001 "$base/tls/cert.pem" "$base/tls/key.pem"
if [[ -z "${SYLUX_TEST_IMAGE:-}" ]]; then
  docker buildx build --platform linux/amd64 --load --tag "$image" .
fi
[[ "$(docker image inspect --format '{{.Os}}/{{.Architecture}}' "$image")" == linux/amd64 ]]
client_image=$(awk '$1 == "FROM" && $2 ~ /^node:/ {print $2; exit}' Dockerfile)
[[ "$client_image" == *@sha256:* ]] || { echo 'Dockerfile must pin its Node base image by digest' >&2; exit 1; }
docker pull "$client_image"
compose() {
  local root=$1
  shift
  SYLUX_IMAGE="$image" SYLUX_DATA_DIR="$root" SYLUX_TLS_DIR="$base/tls" \
    docker compose -f deploy/docker/compose.yaml -p "$name" "$@"
}
compose "$base/root" run --rm --no-deps sylux init --root /var/lib/sylux
start() {
  local root=$1
  compose "$root" run -d --name "$name" --no-deps sylux serve --root /var/lib/sylux --listen 127.0.0.1:33081 --tls-cert /run/sylux/tls/cert.pem --tls-key /run/sylux/tls/key.pem >/dev/null
  wait_ready
}
wait_ready() {
  for _ in {1..30}; do
    if sudo curl --fail --silent --cacert "$base/tls/cert.pem" https://127.0.0.1:33081/readyz >/dev/null; then return; fi
    sleep 1
  done
  docker logs "$name" >&2
  return 1
}
verify_root() {
  sudo "$(command -v node)" scripts/syl7-linux-client.mjs verify "$base/tls/cert.pem" https://127.0.0.1:33081 "$1" "$base/state.json"
}
start "$base/root"
sudo "$(command -v node)" scripts/syl7-linux-client.mjs prepare "$base/tls/cert.pem" https://127.0.0.1:33081 "$base/root" "$base/state.json"
docker stop -t 20 "$name" >/dev/null
[[ "$(docker inspect -f '{{.State.ExitCode}}' "$name")" == 0 ]]
docker start "$name" >/dev/null
wait_ready
verify_root "$base/root"
echo 'clean stop/restart on the same container passed'
docker stop -t 20 "$name" >/dev/null
[[ "$(docker inspect -f '{{.State.ExitCode}}' "$name")" == 0 ]]
docker rm "$name" >/dev/null
start "$base/root"
verify_root "$base/root"
echo 'replacement container on the original root passed'
docker stop -t 20 "$name" >/dev/null
[[ "$(docker inspect -f '{{.State.ExitCode}}' "$name")" == 0 ]]
docker rm "$name" >/dev/null

# Forward HTTPS from a network-isolated client through a pinned SSH daemon.
bridge_ip=$(ip -4 -o addr show docker0 | awk '{split($4, a, "/"); print a[1]}')
[[ -n "$bridge_ip" ]] || { echo 'Docker bridge has no IPv4 address' >&2; exit 1; }
command -v /usr/sbin/sshd >/dev/null || { echo 'Install openssh-server before running this qualification' >&2; exit 1; }
ssh_user=$(id -un)
mkdir -m 700 "$base/ssh"
ssh-keygen -q -t ed25519 -N '' -f "$base/ssh/host_key"
ssh-keygen -q -t ed25519 -N '' -f "$base/ssh/client_key"
printf 'restrict,port-forwarding,permitopen="127.0.0.1:33081" %s\n' "$(cat "$base/ssh/client_key.pub")" > "$base/ssh/authorized_keys"
chmod 600 "$base/ssh/host_key" "$base/ssh/client_key" "$base/ssh/authorized_keys"
ssh-keygen -y -f "$base/ssh/host_key" | awk -v host="[host.docker.internal]:$ssh_port" '{print host, $1, $2}' > "$base/ssh/known_hosts"
cat > "$base/ssh/sshd_config" <<EOF
Port $ssh_port
ListenAddress $bridge_ip
HostKey $base/ssh/host_key
PidFile $base/ssh/sshd.pid
AuthorizedKeysFile $base/ssh/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
UsePAM yes
AllowUsers $ssh_user
PubkeyAuthentication yes
AllowTcpForwarding local
PermitOpen 127.0.0.1:33081
X11Forwarding no
AllowAgentForwarding no
PermitTTY no
EOF
sudo install -d -m 0755 /run/sshd
sudo /usr/sbin/sshd -t -f "$base/ssh/sshd_config"
sudo /usr/sbin/sshd -f "$base/ssh/sshd_config"
[[ "$(sudo ss -ltnH "sport = :$ssh_port" | awk '{print $4}')" == "$bridge_ip:$ssh_port" ]]
start "$base/root"
docker run --rm --name "$client_name" --network bridge --add-host host.docker.internal:host-gateway \
  -e "SSH_PORT=$ssh_port" -e "SSH_USER=$ssh_user" \
  --mount "type=bind,src=$base/ssh/client_key,dst=/run/client_key,readonly" \
  --mount "type=bind,src=$base/ssh/known_hosts,dst=/run/known_hosts,readonly" \
  --mount "type=bind,src=$base/tls/cert.pem,dst=/run/cert.pem,readonly" \
  --mount "type=bind,src=$base/state.json,dst=/run/state.json,readonly" \
  --mount "type=bind,src=$PWD/scripts/syl7-linux-client.mjs,dst=/run/client.mjs,readonly" \
  --entrypoint sh "$client_image" -ec \
  'apt-get update >/dev/null && apt-get install --yes --no-install-recommends openssh-client >/dev/null && ssh -fNT -L 127.0.0.1:33081:127.0.0.1:33081 -p "$SSH_PORT" -i /run/client_key -o IdentitiesOnly=yes -o UserKnownHostsFile=/run/known_hosts -o StrictHostKeyChecking=yes -o ExitOnForwardFailure=yes "$SSH_USER@host.docker.internal" && node /run/client.mjs verify /run/cert.pem https://127.0.0.1:33081 /unused /run/state.json'
echo 'isolated SSH-forwarded HTTPS client passed'
docker stop -t 20 "$name" >/dev/null
[[ "$(docker inspect -f '{{.State.ExitCode}}' "$name")" == 0 ]]
docker rm "$name" >/dev/null
sudo tar -C "$base/root" -cpf "$base/backup.tar" .
sudo tar -tf "$base/backup.tar" >/dev/null
sudo tar -C "$base/restored" --same-owner --same-permissions -xpf "$base/backup.tar"
start "$base/restored"
verify_root "$base/restored"
echo 'stopped-service tar restore to a clean bind mount passed'
