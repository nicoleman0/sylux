#!/usr/bin/env bash
set -Eeuo pipefail
umask 077
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
# shellcheck source=scripts/release-bundle.sh
source scripts/release-bundle.sh

[[ $# == 5 ]] || fail 'Usage: scripts/package-native.sh DOCKER_BUNDLE VERSION SOURCE_SHA DOCKER_MANIFEST_SHA256 NEW_OUTPUT_DIRECTORY'
docker_bundle=$1 version=$2 revision=$3 docker_manifest=$4 output=$5
[[ "$docker_manifest" =~ ^[0-9a-f]{64}$ ]] || fail 'Provide the qualified Docker manifest SHA-256.'
[[ ! -e "$output" && ! -L "$output" ]] || fail 'Native output already exists.'
verify_release_bundle "$docker_bundle" "$version" "$revision" "$docker_manifest"

# The loaded ID may differ from image-id.txt between Docker image stores.
image=$(load_release_image "$docker_bundle" "$version" "$revision")
stage=$(mktemp -d "${TMPDIR:-/tmp}/sylux-native.XXXXXXXX")
container=
trap 'if [[ -n "$container" ]]; then docker_local rm "$container" >/dev/null; fi; rm -rf -- "$stage"' EXIT
container=$(docker_local create --platform linux/amd64 "$image")
mkdir "$stage/payload" "$stage/native"
docker_local cp "$container:/usr/local/bin/sylux" "$stage/payload/sylux"
docker_local cp "$container:/usr/share/doc/sylux/LICENSE" "$stage/payload/LICENSE"
docker_local cp "$container:/usr/share/doc/sylux/third-party" "$stage/payload/third-party"
metadata=$(<"$docker_bundle/release.json")
python3 - "$stage/payload/native-release.json" "$metadata" <<'PY'
import json
from pathlib import Path
import sys

metadata = json.loads(sys.argv[2])
keys = ("version", "source_commit", "source_url", "image_id", "platform")
Path(sys.argv[1]).write_text(json.dumps({key: metadata[key] for key in keys}, indent=2) + "\n")
PY

archive="sylux-$version-linux-amd64.tar.gz"
python3 - "$stage/payload" "$stage/native/$archive" <<'PYTHON'
import gzip
from pathlib import Path
import sys
import tarfile

payload, output = map(Path, sys.argv[1:])


def normalize(member):
    member.mtime = member.uid = member.gid = 0
    member.uname = member.gname = ""
    member.mode = 0o755 if member.isdir() or member.name == "sylux" else 0o644
    return member


with output.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as zipped:
    with tarfile.open(fileobj=zipped, mode="w") as package:
        for path in sorted(payload.iterdir()):
            package.add(path, arcname=path.name, filter=normalize)
PYTHON
helper=deploy/lxc/install/sylux-install.sh
# Release packaging requires the installer from the same exact checkout.
# Historical Docker input is supported for private rehearsal only.
if [[ "$(git rev-parse HEAD)" == "$revision" ]]; then
  git show "$revision:$helper" > "$stage/native/install-sylux-lxc.sh"
elif [[ "${SYLUX_NATIVE_HISTORICAL_FIXTURE:-}" == 1 ]]; then
  cp -- "$helper" "$stage/native/install-sylux-lxc.sh"
else
  fail 'Native installer checkout does not match the release source.'
fi
(
  cd "$stage/native"
  sha256sum -- install-sylux-lxc.sh "$archive" > SHA256SUMS-native
)
digest=$(sha256sum "$stage/native/SHA256SUMS-native")
bash "$helper" verify "$stage/native" "$version" "$revision" "${digest%% *}"
# Claim the output only after every asset has been checked.
mkdir -p -- "$(dirname -- "$output")"
mkdir -- "$output"
cp -- "$stage/native/"* "$output/"
