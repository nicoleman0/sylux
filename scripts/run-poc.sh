#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

usage() {
  cat <<'EOF'
Usage:
  scripts/run-poc.sh init --root DIR
  scripts/run-poc.sh serve --root DIR [--bind ADDRESS:PORT]

Serve defaults to 127.0.0.1:3000. A non-loopback --bind is permitted only
for private/LAN addresses and explicitly enables --allow-non-loopback.
EOF
}

command_name="${1:-}"
case "$command_name" in init|serve) shift ;; *) usage >&2; exit 2 ;; esac
root=""
bind="127.0.0.1:3000"
while (($#)); do
  case "$1" in
    --root) (($# >= 2)) || { usage >&2; exit 2; }; root="$2"; shift 2 ;;
    --bind) (($# >= 2)) || { usage >&2; exit 2; }; bind="$2"; shift 2 ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done
[[ -n "$root" ]] || { printf '%s\n' '--root is required' >&2; usage >&2; exit 2; }

case "$command_name" in
  init)
    cargo build --locked --release --bin sylux
    exec ./target/release/sylux init --root "$root"
    ;;
  serve)
    command -v openssl >/dev/null || { printf '%s\n' 'openssl is required to generate the self-signed TLS identity' >&2; exit 127; }
    [[ -f web/dist/index.html ]] || { printf '%s\n' 'compiled web assets missing; run npm --prefix web run build first' >&2; exit 1; }
    cargo build --locked --release --bin sylux
    mkdir -p -- "$root/tls"
    chmod 700 "$root/tls"
    if [[ ! -s "$root/tls/cert.pem" || ! -s "$root/tls/key.pem" ]]; then
      openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 365 \
        -subj '/CN=localhost' -addext 'subjectAltName=DNS:localhost,IP:127.0.0.1' \
        -keyout "$root/tls/key.pem" -out "$root/tls/cert.pem" >/dev/null 2>&1
      chmod 600 "$root/tls/key.pem" "$root/tls/cert.pem"
    fi
    args=(serve --root "$root" --listen "$bind" --tls-cert "$root/tls/cert.pem" --tls-key "$root/tls/key.pem")
    if [[ "$bind" != 127.0.0.1:* && "$bind" != '[::1]:'* ]]; then
      args+=(--allow-non-loopback)
    fi
    exec ./target/release/sylux "${args[@]}"
    ;;
esac
