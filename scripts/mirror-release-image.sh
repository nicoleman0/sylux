#!/usr/bin/env bash
# shellcheck source=scripts/release-bundle.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/release-bundle.sh"

mirror_log() {
  printf '%s\n' "$*" | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
}

mirror_config() {
  local repository=$1 username=$2 token=$3
  if [[ -z "$repository$username$token" ]]; then
    mirror_log 'Docker Hub mirror skipped. No mirror settings are configured.'
    printf 'enabled=false\n' >> "$GITHUB_OUTPUT"
    return
  fi
  [[ "$repository" =~ ^[a-z0-9]+([_-][a-z0-9]+)*/[a-z0-9]+([._-][a-z0-9]+)*$ &&
    "$username" =~ ^[a-z0-9][a-z0-9_-]*$ && -n "$token" ]] ||
    fail 'Set SYLUX_DOCKER_HUB_REPOSITORY (namespace/repository), SYLUX_DOCKER_HUB_USERNAME and SYLUX_DOCKER_HUB_PUSH_TOKEN together.'
  printf 'enabled=true\n' >> "$GITHUB_OUTPUT"
}

tag_digest() {
  local result
  if result=$(crane digest "$1" 2>&1); then
    result=${result##*$'\n'}
    [[ "$result" =~ ^sha256:[a-f0-9]{64}$ ]] || fail "Invalid registry digest for $1."
    printf '%s\n' "$result"
  elif [[ "$result" != *$'\nError: GET '*': MANIFEST_UNKNOWN:'* &&
    "$result" != *$'\nError: GET '*': NAME_UNKNOWN:'* ]]; then
    fail "Registry lookup failed for $1. $result"
  fi
}

image_config_digest() {
  crane manifest "$1" | jq -er '
    select(.mediaType == "application/vnd.docker.distribution.manifest.v2+json"
      or .mediaType == "application/vnd.oci.image.manifest.v1+json")
    | .config.digest | select(test("^sha256:[0-9a-f]{64}$"))'
}

verify_public_tag() {
  local actual
  actual=$(DOCKER_CONFIG="$anonymous_config" crane digest "$1")
  [[ "$actual" == "$2" ]] || fail "Public digest mismatch for $1. Expected $2, got $actual."
}

mirror_image() {
  local source_repo=$1 destination=$2 version=$3 revision=$4 image_id=$5
  local exact="$destination:$version" alias source source_digest config exact_digest
  local alias_digest alias_version version_order
  alias=$(release_alias "$version") || fail 'Invalid release version.'
  source_digest=$(DOCKER_CONFIG="$anonymous_config" crane digest "$source_repo:$version")
  [[ "$source_digest" =~ ^sha256:[a-f0-9]{64}$ ]] || fail 'Invalid source manifest digest.'
  source="$source_repo@$source_digest"
  [[ "$image_id" =~ ^sha256:[a-f0-9]{64}$ &&
    "$(DOCKER_CONFIG="$anonymous_config" image_config_digest "$source")" == "$image_id" ]] ||
    fail 'Registry source does not match the qualified image ID.'
  config=$(DOCKER_CONFIG="$anonymous_config" crane config "$source")
  jq -e --arg version "$version" --arg revision "$revision" '
    .os == "linux" and .architecture == "amd64"
    and .config.Labels["org.opencontainers.image.source"] == "https://github.com/nicoleman0/sylux"
    and .config.Labels["org.opencontainers.image.version"] == $version
    and .config.Labels["org.opencontainers.image.revision"] == $revision' <<< "$config" >/dev/null ||
    fail 'Unexpected platform, version, revision or source on the registry image.'

  exact_digest=$(tag_digest "$exact")
  if [[ -z "$exact_digest" ]]; then
    crane copy --no-clobber "$source" "$exact"
  elif [[ "$exact_digest" != "$source_digest" ]]; then
    fail "Exact version conflict at $exact. Expected $source_digest, found $exact_digest."
  fi
  verify_public_tag "$exact" "$source_digest"
  mirror_log "Verified $exact at $source_digest (qualified image $image_id)."

  alias="$destination:$alias"
  alias_digest=$(tag_digest "$alias")
  if [[ -n "$alias_digest" && "$alias_digest" != "$source_digest" ]]; then
    if ! alias_version=$(crane config "$destination@$alias_digest" | jq -er '
      select(.config.Labels["org.opencontainers.image.source"] == "https://github.com/nicoleman0/sylux")
      | .config.Labels["org.opencontainers.image.version"]'); then
      fail "Existing $alias is not a labeled Sylux release image; leaving it unchanged."
    fi
    [[ "$(release_alias "$alias_version")" == "${alias##*:}" &&
      "$(crane digest "$destination:$alias_version")" == "$alias_digest" ]] ||
      fail "Existing $alias cannot be matched to an exact version on its channel."
    version_order=$(python3 - "$version" "$alias_version" <<'PY'
import re
import sys

candidate, current = (tuple(map(int, re.findall(r"[0-9]+", tag))) for tag in sys.argv[1:])
print((candidate > current) - (candidate < current))
PY
    )
    if [[ "$version_order" == -1 ]]; then
      verify_public_tag "$alias" "$alias_digest"
      mirror_log "Kept newer $alias at $alias_version ($alias_digest)."
      return
    fi
    [[ "$version_order" == 1 ]] || fail "Existing $alias has a conflicting digest at the same release version."
  fi
  if [[ "$alias_digest" != "$source_digest" ]]; then
    crane tag "$destination@$source_digest" "${alias##*:}"
  fi
  verify_public_tag "$alias" "$source_digest"
  mirror_log "Verified $alias at $source_digest."
}

publish_mirror() {
  local image_id manifest
  check_release_publish_ref "$RELEASE_VERSION" || fail 'Release tag is not authorized.'
  [[ "$QUALIFIED_MANIFEST" =~ ^[a-f0-9]{64}$ ]] || fail 'Missing qualified bundle checksum.'
  work=$(mktemp -d)
  trap 'rm -rf -- "${work:?}"' EXIT
  mkdir "$work/public" "$work/private"
  local anonymous_config="$work/public"
  export DOCKER_CONFIG="$work/private"
  printf '{}\n' > "$anonymous_config/config.json"
  printf '{}\n' > "$DOCKER_CONFIG/config.json"

  download "$RELEASE_VERSION" "$work" SHA256SUMS
  manifest=$(sha256sum "$work/SHA256SUMS")
  [[ "${manifest%% *}" == "$QUALIFIED_MANIFEST" ]] || fail 'Public checksums differ from qualification.'
  download "$RELEASE_VERSION" "$work" image-id.txt
  (cd "$work" && awk '$2 == "image-id.txt"' SHA256SUMS | sha256sum --check --strict -)
  image_id=$(cat "$work/image-id.txt")

  printf '%s' "$MIRROR_TOKEN" | crane auth login index.docker.io --username "$MIRROR_USERNAME" --password-stdin
  unset MIRROR_TOKEN
  mirror_image "ghcr.io/$GITHUB_REPOSITORY" "index.docker.io/$MIRROR_REPOSITORY" \
    "$RELEASE_VERSION" "$GITHUB_SHA" "$image_id"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-}" in
    config) mirror_config "$MIRROR_REPOSITORY" "$MIRROR_USERNAME" "$MIRROR_TOKEN" ;;
    publish) publish_mirror ;;
    *) fail 'Expected config or publish.' ;;
  esac
fi
