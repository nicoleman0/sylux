#!/usr/bin/env bash
set -Eeuo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
for script in deploy/lxc/{ct/sylux,install/sylux-install}.sh \
  scripts/{check-native-syntax,package-native,qualify-native-lxc,test-native}.sh; do
  bash -n "$script"
done
