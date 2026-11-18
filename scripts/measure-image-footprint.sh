#!/usr/bin/env bash
# Measures a published Sylux image: size, start-to-ready, idle and workload memory.
# Usage: scripts/measure-image-footprint.sh ghcr.io/nicoleman0/sylux@sha256:<digest>
# Needs Docker, Node, openssl and root on a Linux host with cgroup v2. Uses disposable data.
set -euo pipefail

image=${1:?usage: $0 <registry/repo@sha256:digest>}
[[ "$image" =~ @sha256:[0-9a-f]{64}$ ]] || { echo 'Pass the image by digest, not by tag' >&2; exit 1; }
[[ "$(uname -s)" == Linux ]] || { echo 'Requires Linux' >&2; exit 1; }
[[ "$(uname -m)" == x86_64 || "${SYLUX_ALLOW_NON_X86:-}" == 1 ]] || { echo 'Requires x86_64 (SYLUX_ALLOW_NON_X86=1 overrides and invalidates the result)' >&2; exit 1; }
[[ "$(id -u)" == 0 ]] || { echo 'Run as root: the data and TLS directories belong to UID 10001' >&2; exit 1; }
[[ -f /sys/fs/cgroup/cgroup.controllers ]] || { echo 'Requires cgroup v2' >&2; exit 1; }

runs=${RUNS:-5} idle=${IDLE_SECONDS:-30} files=${FILES:-40} file_mib=${FILE_MIB:-4} workers=${WORKERS:-4}
here=$(cd "$(dirname "$0")" && pwd)
base=$(mktemp -d "${TMPDIR:-/var/tmp}/sylux-footprint.XXXXXXXX")
name="sylux-footprint-$$"
poller=
cleanup() {
  if [[ -n "$poller" ]]; then kill "$poller" 2>/dev/null || true; wait "$poller" 2>/dev/null || true; fi
  docker rm -f "$name" >/dev/null 2>&1 || true
  rm -rf -- "$base"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

mkdir -m 0700 "$base/root" "$base/tls"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj '/CN=127.0.0.1' -addext 'subjectAltName=IP:127.0.0.1' \
  -keyout "$base/tls/key.pem" -out "$base/tls/cert.pem" >/dev/null 2>&1
chmod 0644 "$base/tls/cert.pem"; chmod 0600 "$base/tls/key.pem"
chown -R 10001:10001 "$base/root" "$base/tls"

docker pull --quiet --platform linux/amd64 "$image" >/dev/null
[[ "$(docker image inspect --format '{{.Os}}/{{.Architecture}}' "$image")" == linux/amd64 ]]
unpacked=$(docker image inspect --format '{{.Size}}' "$image")
compressed=$(docker buildx imagetools inspect "$image" --raw | node -e 'const m=JSON.parse(require("fs").readFileSync(0));console.log(m.layers.reduce((n,l)=>n+l.size,0))')

# Same hardening as deploy/docker/compose.yaml; Compose itself is not required.
run_args=(--read-only --init --cap-drop ALL --security-opt no-new-privileges:true --network host --user 10001:10001
  --mount "type=bind,src=$base/root,dst=/var/lib/sylux"
  --mount "type=bind,src=$base/tls,dst=/run/sylux/tls,readonly")
docker run --rm --platform linux/amd64 "${run_args[@]}" "$image" init --root /var/lib/sylux >/dev/null
docker create --name "$name" --platform linux/amd64 "${run_args[@]}" "$image" >/dev/null

origin=https://127.0.0.1:3000
starts=()
for ((n = 1; n <= runs; n++)); do
  node "$here/measure-image-footprint.mjs" ready "$origin" "$base/tls/cert.pem" >"$base/ready" &
  poller=$!
  docker start "$name" >/dev/null
  wait "$poller" || { echo "Container did not become ready within ${READY_TIMEOUT_MS:-30000} ms" >&2; exit 1; }
  poller=
  started_ns=$(docker inspect --format '{{.State.StartedAt}}' "$name" | xargs node "$here/measure-image-footprint.mjs" epoch-ms)
  starts+=("$(node -e 'console.log((Number(process.argv[1])-Number(process.argv[2])).toFixed(1))' "$(cat "$base/ready")" "$started_ns")")
  [[ $n == "$runs" ]] || docker stop --time 20 "$name" >/dev/null
done

init_pid=$(docker inspect --format '{{.State.Pid}}' "$name")
pid=$(pgrep -P "$init_pid")  # --init makes PID 1 a reaper; measure its child, the server
[[ "$(cat "/proc/$pid/comm")" == sylux ]] || { echo 'Server process not found' >&2; exit 1; }
cgroup=$(sed -n 's/^0:://p' "/proc/$init_pid/cgroup")
load=$(node "$here/measure-image-footprint.mjs" run "$origin" "$base/tls/cert.pem" "$pid" "$cgroup" "$base/root/secrets/setup.authorization" "$idle" "$files" "$file_mib" "$workers")

SYLUX_IMAGE=$image SYLUX_UNPACKED=$unpacked SYLUX_COMPRESSED=$compressed SYLUX_STARTS="${starts[*]}" SYLUX_LOAD=$load \
SYLUX_HOST="$(uname -srm); $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | xargs); $(nproc) CPUs; $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo); $(stat -f -c %T "$base/root")" \
SYLUX_DOCKER="$(docker version --format '{{.Server.Version}}')" SYLUX_DATE="$(date -u +%FT%TZ)" \
node -e '
const e = process.env;
console.log(JSON.stringify({
  image: e.SYLUX_IMAGE, measuredAt: e.SYLUX_DATE, host: e.SYLUX_HOST, docker: e.SYLUX_DOCKER,
  imageBytes: { unpacked: Number(e.SYLUX_UNPACKED), compressed: Number(e.SYLUX_COMPRESSED) },
  startToReadyMs: e.SYLUX_STARTS.split(" ").map(Number),
  ...JSON.parse(e.SYLUX_LOAD),
}, null, 2));
'
