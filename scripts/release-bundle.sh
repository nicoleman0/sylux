#!/usr/bin/env bash
# shellcheck source=scripts/install-sylux.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/install-sylux.sh"

verify_release_bundle() {
  local directory=$1 version=$2 revision=$3 expected_hash=${4:-} manifest
  [[ "$revision" =~ ^[a-f0-9]{40}$ ]] || fail 'Expected the full release source SHA.'
  verify_bundle "$directory" "$version" "$revision" >/dev/null || return 1
  manifest=$(sha256sum "$directory/SHA256SUMS") || return 1
  manifest=${manifest%% *}
  if [[ -n "$expected_hash" && "$manifest" != "$expected_hash" ]]; then
    echo 'Bundle SHA256SUMS does not match the qualified bundle.' >&2
    return 1
  fi
  printf '%s\n' "$manifest"
}

load_release_image() {
  local directory=$1 version=$2 revision=$3 tag="sylux:$2" existing id
  existing=$(docker_local image ls -q "$tag") || return 1
  [[ -z "$existing" ]] || fail 'The release tag is already loaded locally.'
  docker_local load --input "$directory/sylux-image.tar.gz" >/dev/null || return 1
  id=$(inspect_image "$tag" "$version" "$revision") || return 1
  [[ "$(docker_local image inspect --format '{{index .Config.Labels "org.opencontainers.image.source"}}' "$id")" == \
    https://github.com/nicoleman0/sylux ]] || fail 'Unexpected image source label.'
  printf '%s\n' "$id"
}

release_alias() {
  local channel
  channel=$(release_channel "$1") || return 1
  case "$channel" in
    stable) printf 'latest\n' ;;
    beta) printf 'beta\n' ;;
  esac
}

check_release_publish_ref() {
  [[ "${GITHUB_EVENT_NAME:-}" == push &&
    "${GITHUB_REPOSITORY:-}" == nicoleman0/sylux &&
    "${GITHUB_ACTOR:-}" == nicoleman0 &&
    "${GITHUB_TRIGGERING_ACTOR:-}" == nicoleman0 &&
    "${GITHUB_REF:-}" == "refs/tags/$1" ]] &&
    release_channel "$1" >/dev/null
}
