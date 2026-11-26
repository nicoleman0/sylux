#!/bin/sh

fail() {
    printf 'Sylux install: %s\n' "$*" >&2
    exit 1
}

download() {
    curl -qfsSL --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time 60 "$@"
}

installer_digest() {
    command -v python3 >/dev/null || fail 'Python 3 is required to check the release.'
    release=$(download -H 'Accept: application/vnd.github+json' "$1") ||
        fail "Could not confirm $2. Check the releases page and retry."
    printf '%s' "$release" | python3 -c '
import json
import re
import sys

version, url, channel = sys.argv[1:]
prerelease = channel == "beta"
try:
    release = json.load(sys.stdin)
    assets = [asset for asset in release["assets"] if asset["name"] == "install-sylux.sh"]
    ready = (
        release["tag_name"] == version
        and release["draft"] is False
        and release["prerelease"] is prerelease
        and len(assets) == 1
        and assets[0]["state"] == "uploaded"
        and type(assets[0]["size"]) is int
        and assets[0]["size"] > 0
        and assets[0]["browser_download_url"] == url
        and re.fullmatch(r"sha256:[a-f0-9]{64}", assets[0]["digest"])
    )
except (KeyError, TypeError, ValueError):
    ready = False
if not ready:
    sys.exit(1)
print(assets[0]["digest"])
' "$2" "$3" "$4" ||
        fail "GitHub did not report the expected $4 installer for $2. Check the releases page and retry."
}

install_sylux() (
    set -eu
    api=https://api.github.com/repos/nicoleman0/sylux/releases
    case "$#:${1-}" in
        0:)
            version=v0.1.0
            api="$api/latest"
            channel=stable
            ;;
        1:--beta)
            version=v0.1.0-beta.10
            api="$api/tags/$version"
            channel=beta
            ;;
        *) fail 'Run with no arguments for stable or pass --beta for v0.1.0-beta.10.' ;;
    esac
    url="https://github.com/nicoleman0/sylux/releases/download/$version/install-sylux.sh"
    digest=$(installer_digest "$api" "$version" "$url" "$channel")
    installer=$(mktemp)
    trap 'rm -f "$installer"' EXIT
    download --output "$installer" "$url" && [ -s "$installer" ] ||
        fail "Could not download the installer for $version."
    actual=$(sha256sum <"$installer")
    [ "sha256:${actual%% *}" = "$digest" ] ||
        fail "The installer checksum did not match for $version."
    if ! ( : </dev/tty ) 2>/dev/null; then
        fail 'Run the launcher in an interactive terminal.'
    fi
    exec </dev/tty
    sudo bash "$installer" "$version"
)

install_sylux "$@"
