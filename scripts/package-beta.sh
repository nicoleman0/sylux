#!/usr/bin/env bash
set -Eeuo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
version=${1:?Usage: scripts/package-beta.sh vX.Y.Z-beta.N NEW_OUTPUT_DIRECTORY}
output=${2:?Provide a new output directory}
[[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-beta\.[1-9][0-9]*$ ]] || { echo 'Expected vX.Y.Z-beta.N' >&2; exit 1; }
git diff --quiet HEAD -- || { echo 'Commit tracked changes before packaging' >&2; exit 1; }
[[ ! -e "$output" ]] || { echo 'Output directory already exists' >&2; exit 1; }
revision=$(git rev-parse HEAD)
context=$(mktemp -d "${TMPDIR:-/tmp}/sylux-package.XXXXXXXX")
trap 'rm -rf -- "$context"' EXIT
git archive HEAD | tar -x -C "$context"
mkdir -p -- "$output"
output=$(cd "$output" && pwd)
image="sylux:$version"
docker buildx build --platform linux/amd64 --provenance=false --load --tag "$image" \
  --label "org.opencontainers.image.source=https://github.com/nicoleman0/sylux" \
  --label "org.opencontainers.image.revision=$revision" \
  --label "org.opencontainers.image.version=$version" \
  --label 'org.opencontainers.image.licenses=MIT' "$context"
image_id=$(docker image inspect --format '{{.Id}}' "$image")
[[ "$image_id" =~ ^sha256:[a-f0-9]{64}$ ]] || { echo 'Invalid Docker image ID' >&2; exit 1; }
[[ "$(docker image inspect --format '{{.Os}}/{{.Architecture}}' "$image_id")" == linux/amd64 ]]
docker save "$image" | gzip -n > "$output/sylux-image.tar.gz"
printf '%s\n' "$image_id" > "$output/image-id.txt"
cp "$context/deploy/docker/compose.yaml" "$context/deploy/docker/.env.example" "$context/docs/docker-beta.md" "$output/"
python3 - "$output" "$version" "$revision" "$image_id" <<'PY'
import hashlib, json, pathlib, sys
out, version, revision, image_id = sys.argv[1:]
folder = pathlib.Path(out)
(folder / 'release.json').write_text(json.dumps({
    'version': version, 'source_commit': revision,
    'source_url': f'https://github.com/nicoleman0/sylux/tree/{revision}',
    'image_id': image_id, 'platform': 'linux/amd64',
    'qualification': 'See release workflow; packaging alone is not acceptance.'
}, indent=2) + '\n')
with (folder / 'SHA256SUMS').open('w') as sums:
    for path in sorted(folder.iterdir()):
        if path.name == 'SHA256SUMS':
            continue
        with path.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        sums.write(f'{digest}  {path.name}\n')
PY
printf 'Packaged %s (%s) in %s\n' "$version" "$image_id" "$output"
