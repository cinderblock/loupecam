#!/usr/bin/env bash
# Publish the library crates to crates.io, in dependency order. A crate whose version
# is already there is skipped, so re-running a release (workflow_dispatch) is safe.
# Needs CARGO_REGISTRY_TOKEN, and set-version.sh to have run (the tree is dirty then).
set -euo pipefail
v="${VERSION:?VERSION must be set (set-version.sh)}"
for crate in loupecam-protocol loupecam loupecam-isp; do
  code=$(curl -s -o /dev/null -w '%{http_code}' -A 'loupecam-release (github.com/cinderblock/loupecam)' \
    "https://crates.io/api/v1/crates/$crate/$v")
  if [ "$code" = 200 ]; then
    echo "$crate $v is already on crates.io; skipping"
    continue
  fi
  cargo publish --locked --allow-dirty -p "$crate"
done
