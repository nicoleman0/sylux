#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
image=quay.io/keycloak/keycloak:26.6.4@sha256:0aae0de7fca85525f727d3354df17896092de8bb26ae4c12d89c77e5df8cbce4

if [[ ! -f "$root/web/node_modules/playwright/package.json" ]]; then
    echo 'Install the locked web dependencies and Chromium first.' >&2
    exit 1
fi
docker info --format 'Docker daemon {{.ServerVersion}} {{.OSType}}/{{.Architecture}}'
umask 077
fixture=$(mktemp -d)
name="syl114-$(openssl rand -hex 12)"
container=
network=

cleanup() {
    local status=$?
    trap - EXIT
    if [[ -n "$container" ]] && ! docker rm --force --volumes "$container" >/dev/null 2>&1; then
        echo "Cleanup failed for fixture container $name" >&2
        status=1
    fi
    if [[ -n "$network" ]] && ! docker network rm "$network" >/dev/null 2>&1; then
        echo "Cleanup failed for fixture network $name" >&2
        status=1
    fi
    if [[ -n "${sylux_fixture:-}" ]]; then
        if ! rm -rf -- "$fixture/sylux" "$fixture/sylux-oidc.json" "$fixture/oidc-client-secret" "$fixture/backup"; then
            echo 'Cleanup failed for disposable Sylux storage.' >&2
            status=1
        fi
    fi
    if ! rm -f -- "$fixture/"{ca.key,ca.pem,ca.srl,leaf.csr,leaf.ext,untrusted.key,untrusted.pem} \
        "$fixture/tls/"{leaf.key,leaf.pem} || ! rmdir "$fixture/tls" "$fixture"; then
        echo 'Cleanup failed for fixture files.' >&2
        status=1
    fi
    if (( status == 0 )); then
        echo 'Cleanup passed (container, network, temporary files).'
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Give the container read access to the leaf key inside the private host directory.
mkdir -m 755 "$fixture/tls"
openssl req -x509 -newkey rsa:2048 -noenc -days 1 -sha256 -subj '/CN=SYL-114 fixture CA' \
    -keyout "$fixture/ca.key" -out "$fixture/ca.pem" \
    -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign' >/dev/null 2>&1
openssl req -new -newkey rsa:2048 -noenc -subj '/CN=127.0.0.1' \
    -keyout "$fixture/tls/leaf.key" -out "$fixture/leaf.csr" >/dev/null 2>&1
cat > "$fixture/leaf.ext" <<'EXT'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=IP:127.0.0.1,DNS:localhost
EXT
openssl x509 -req -in "$fixture/leaf.csr" -CA "$fixture/ca.pem" -CAkey "$fixture/ca.key" \
    -CAcreateserial -out "$fixture/tls/leaf.pem" -days 1 -sha256 -extfile "$fixture/leaf.ext" >/dev/null 2>&1
openssl verify -CAfile "$fixture/ca.pem" -verify_ip 127.0.0.1 "$fixture/tls/leaf.pem" >/dev/null
openssl req -x509 -newkey rsa:2048 -noenc -days 1 -sha256 -subj '/CN=Untrusted fixture CA' \
    -keyout "$fixture/untrusted.key" -out "$fixture/untrusted.pem" \
    -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign' >/dev/null 2>&1
chmod 644 "$fixture/tls/"{leaf.key,leaf.pem}

export KC_BOOTSTRAP_ADMIN_CLIENT_SECRET
KC_BOOTSTRAP_ADMIN_CLIENT_SECRET=$(openssl rand -hex 32)
port=$(node --input-type=module <<'JS'
import { createServer } from 'node:net';

const socket = createServer().listen(0, '127.0.0.1', () => {
    console.log(socket.address().port);
    socket.close();
});
JS
)
# Keep the same issuer when stopping and restarting the container.
binding="127.0.0.1:$port"
network=$(docker network create --driver bridge --opt com.docker.network.bridge.enable_ip_masquerade=false "$name")
container=$(docker create --name "$name" --network "$network" --log-driver=none \
    --publish "$binding:8443" --env KC_BOOTSTRAP_ADMIN_CLIENT_ID=fixture-admin \
    --env KC_BOOTSTRAP_ADMIN_CLIENT_SECRET --mount "type=bind,src=$fixture/tls,dst=/fixture,readonly" \
    "$image" start-dev --http-enabled=false --hostname=127.0.0.1 \
    --https-certificate-file=/fixture/leaf.pem --https-certificate-key-file=/fixture/leaf.key)
docker start "$container" >/dev/null
if [[ $(docker port "$container" 8443/tcp) != "$binding" ]]; then
    echo 'Expected the configured IPv4 loopback binding.' >&2
    exit 1
fi
sylux_fixture=${1:-}
env -u DEBUG -u PWDEBUG -u NODE_DEBUG -u NODE_DEBUG_NATIVE \
    NODE_EXTRA_CA_CERTS="$fixture/ca.pem" \
    node "$root/scripts/qualify-keycloak.mjs" "$fixture" "$container" "https://$binding" "$sylux_fixture"
