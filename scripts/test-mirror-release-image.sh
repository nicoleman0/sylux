#!/usr/bin/env bash
# shellcheck source=scripts/mirror-release-image.sh
source scripts/mirror-release-image.sh

fixture=$(mktemp -d)
registries=()
cleanup() {
  for pid in "${registries[@]}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf -- "${fixture:?}"
}
trap cleanup EXIT
mkdir "$fixture/auth" "$fixture/anonymous"
export DOCKER_CONFIG="$fixture/auth"
anonymous_config="$fixture/anonymous"
printf '{}\n' > "$anonymous_config/config.json"
printf '{}\n' > "$DOCKER_CONFIG/config.json"

start_registry() {
  local log=$1
  registry -port 0 > "$log" 2>&1 &
  registries+=("$!")
  for _ in {1..100}; do
    if grep -q 'serving on port ' "$log"; then
      registry_address="localhost:$(sed -n 's/.*serving on port //p' "$log")"
      return
    fi
    sleep 0.05
  done
  cat "$log" >&2
  fail 'Fixture registry did not start.'
}

expect_failure() {
  local message=$1
  shift
  local status=0
  bash -c 'source scripts/mirror-release-image.sh; anonymous_config=$1; shift; "$@"' \
    mirror-test "$anonymous_config" "$@" > "$fixture/failed" 2>&1 || status=$?
  if [[ "$status" == 0 ]] || ! grep -Fq "$message" "$fixture/failed"; then
    cat "$fixture/failed" >&2
    fail "Expected failure containing $message."
  fi
}

export GITHUB_OUTPUT="$fixture/settings"
mirror_config '' '' ''
grep -qx 'enabled=false' "$GITHUB_OUTPUT"
mirror_config 'test/sylux' 'test' 'fixture-token'
grep -qx 'enabled=true' "$GITHUB_OUTPUT"
expect_failure 'Set SYLUX_DOCKER_HUB_REPOSITORY' mirror_config 'test/sylux' '' ''
expect_failure 'Set SYLUX_DOCKER_HUB_REPOSITORY' mirror_config '' '' 'fixture-token'
expect_failure 'Set SYLUX_DOCKER_HUB_REPOSITORY' mirror_config 'ghcr.io/test/sylux' 'test' 'fixture-token'
RELEASE_VERSION=v1.0.0 GITHUB_EVENT_NAME=workflow_dispatch \
  expect_failure 'Release tag is not authorized' publish_mirror

start_registry "$fixture/source.log"
source_repo="$registry_address/sylux/source"
start_registry "$fixture/mirror.log"
destination="$registry_address/sylux/mirror"
revision=0000000000000000000000000000000000000001
printf 'fixture bytes\n' > "$fixture/payload"
tar -C "$fixture" -cf "$fixture/layer.tar" payload
crane append -f "$fixture/layer.tar" -t "$source_repo:base"
for version in v1.0.0-beta.2 v1.0.0-beta.10 v1.0.0 v1.1.0; do
  crane mutate "$source_repo:base" --set-platform linux/amd64 \
    --label "org.opencontainers.image.version=$version" \
    --label "org.opencontainers.image.revision=$revision" \
    --label org.opencontainers.image.source=https://github.com/nicoleman0/sylux \
    --tag "$source_repo:$version"
done

publish_fixture() {
  mirror_image "$source_repo" "$destination" "$1" "$revision" \
    "$(image_config_digest "$source_repo:$1")"
}

old_beta=v1.0.0-beta.2
new_beta=v1.0.0-beta.10
stable=v1.0.0
next_stable=v1.1.0
old_digest=$(crane digest "$source_repo:$old_beta")
new_digest=$(crane digest "$source_repo:$new_beta")
stable_digest=$(crane digest "$source_repo:$stable")
next_stable_digest=$(crane digest "$source_repo:$next_stable")

expect_failure 'qualified image ID' mirror_image "$source_repo" "$destination" "$old_beta" \
  "$revision" "$(image_config_digest "$source_repo:$stable")"
[[ -z "$(tag_digest "$destination:$old_beta")" ]]
publish_fixture "$old_beta"
[[ "$(crane ls "$destination" | LC_ALL=C sort)" == "$(printf 'beta\n%s' "$old_beta")" ]]
publish_fixture "$new_beta"
publish_fixture "$old_beta"
publish_fixture "$new_beta"
verify_public_tag "$destination:beta" "$new_digest"
verify_public_tag "$destination:$old_beta" "$old_digest"

publish_fixture "$stable"
verify_public_tag "$destination:latest" "$stable_digest"
verify_public_tag "$destination:beta" "$new_digest"

# Force a conflicting exact version, then a failure after writing the exact version.
conflict="${destination}-conflict"
crane copy "$source_repo:$stable" "$conflict:$next_stable"
expect_failure 'Exact version conflict' mirror_image "$source_repo" "$conflict" "$next_stable" \
  "$revision" "$(image_config_digest "$source_repo:$next_stable")"
verify_public_tag "$conflict:$next_stable" "$stable_digest"
[[ -z "$(tag_digest "$conflict:latest")" ]]

# An unlabeled beta alias must fail with an actionable message and remain untouched.
unlabeled="${destination}-unlabeled"
crane copy "$source_repo:base" "$unlabeled:v9.0.0-beta.99"
crane tag "$unlabeled:v9.0.0-beta.99" beta
unlabeled_digest=$(tag_digest "$unlabeled:beta")
expect_failure 'not a labeled Sylux release image; leaving it unchanged' \
  mirror_image "$source_repo" "$unlabeled" "$new_beta" "$revision" \
  "$(image_config_digest "$source_repo:$new_beta")"
verify_public_tag "$unlabeled:beta" "$unlabeled_digest"
verify_public_tag "$unlabeled:$new_beta" "$new_digest"

crane tag "$destination@$new_digest" latest
expect_failure 'cannot be matched to an exact version on its channel' \
  mirror_image "$source_repo" "$destination" "$next_stable" "$revision" \
  "$(image_config_digest "$source_repo:$next_stable")"
verify_public_tag "$destination:$next_stable" "$next_stable_digest"
verify_public_tag "$destination:latest" "$new_digest"
crane tag "$destination@$stable_digest" latest
publish_fixture "$next_stable"
publish_fixture "$stable"
verify_public_tag "$destination:latest" "$next_stable_digest"
verify_public_tag "$destination:beta" "$new_digest"

crane export "$destination@$next_stable_digest" "$fixture/mirrored.tar"
tar -xOf "$fixture/mirrored.tar" payload | cmp "$fixture/payload" -
echo 'Mirror settings, authorization, identity, alias precedence, conflict and retry checks passed.'
