#!/usr/bin/env bash
set -Eeuo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
# shellcheck source=scripts/release-bundle.sh
source scripts/release-bundle.sh

[[ "$(release_channel v0.1.0)" == stable && "$(release_alias v0.1.0)" == latest ]]
[[ "$(release_channel v0.1.0-beta.10)" == beta && "$(release_alias v0.1.0-beta.10)" == beta ]]
# The installer must accept both versions before doing any host work.
check_host() { exit 42; }
for version in v0.1.0 v0.1.0-beta.10; do
  status=0
  (main "$version" --yes) || status=$?
  [[ "$status" == 42 ]]
done
for invalid in '' 0.1.0 latest beta v0.1 v0.1.0-rc.1 v0.1.0-beta.0 v0.1.0-beta.01 \
  v0.1.0-beta.1/other 'v0.1.0 ' $'v0.1.0\n'; do
  ! release_alias "$invalid" || fail 'Accepted an invalid version or mutable alias.'
  status=0
  (main "$invalid") 2>/dev/null || status=$?
  [[ "$status" == 1 ]]
done
status=0
(main v0.1.0 '') 2>/dev/null || status=$?
[[ "$status" == 1 ]]
echo 'Release channel and installer argument contracts passed.'

GITHUB_EVENT_NAME=push
GITHUB_REPOSITORY=nicoleman0/sylux
GITHUB_ACTOR=nicoleman0
GITHUB_TRIGGERING_ACTOR=nicoleman0
GITHUB_REF=refs/tags/v0.1.0
check_release_publish_ref v0.1.0
GITHUB_REF=refs/tags/v0.1.0-beta.10 check_release_publish_ref v0.1.0-beta.10
reject_publish() {
  if check_release_publish_ref "$1"; then
    fail 'Accepted an unauthorized release event or ref.'
  fi
}
GITHUB_EVENT_NAME=workflow_dispatch reject_publish v0.1.0
GITHUB_REF=refs/heads/main reject_publish v0.1.0
GITHUB_REF=refs/heads/v0.1.0 reject_publish v0.1.0
GITHUB_REF=refs/tags/latest reject_publish latest
GITHUB_REPOSITORY=other/sylux reject_publish v0.1.0
GITHUB_ACTOR=other reject_publish v0.1.0
GITHUB_TRIGGERING_ACTOR=other reject_publish v0.1.0
reject_publish v0.1.1
echo 'Publish event, repository, owner and exact tag contracts passed.'

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
version=v0.1.0
revision=0123456789abcdef0123456789abcdef01234567
image_id=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
for asset in "${release_assets[@]}" install-sylux.sh; do
  printf 'fixture\n' > "$fixture/$asset"
done
printf '%s\n' "$image_id" > "$fixture/image-id.txt"
printf '{"version":"%s","source_commit":"%s","source_url":"https://github.com/nicoleman0/sylux/tree/%s","image_id":"%s","platform":"linux/amd64"}\n' \
  "$version" "$revision" "$revision" "$image_id" > "$fixture/release.json"
(cd "$fixture" && sha256sum -- "${release_assets[@]}" install-sylux.sh > SHA256SUMS)
manifest=$(verify_release_bundle "$fixture" "$version" "$revision")
[[ "$(verify_release_bundle "$fixture" "$version" "$revision" "$manifest")" == "$manifest" ]]

reject() {
  if (verify_release_bundle "$fixture" "$@") >/dev/null 2>&1; then
    fail 'Accepted an altered bundle or mismatched identity.'
  fi
}
reject v0.1.0-beta.10 "$revision" "$manifest"
reject "$version" ffffffffffffffffffffffffffffffffffffffff "$manifest"
reject "$version" "$revision" "$(printf '%064d' 0)"
mkdir "$fixture/unexpected"
reject "$version" "$revision" "$manifest"
rmdir "$fixture/unexpected"
rm "$fixture/install-sylux.sh"
reject "$version" "$revision" "$manifest"
(cd "$fixture" && sha256sum -- "${release_assets[@]}" > SHA256SUMS)
[[ "$(verify_bundle "$fixture" "$version")" == "$revision" ]]
reject "$version" "$revision"
echo 'Exact release bundle and legacy installer asset contracts passed.'
