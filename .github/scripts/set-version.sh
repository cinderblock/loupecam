#!/usr/bin/env bash
# Set the workspace version from the release tag, so pushing vX.Y.Z is the whole
# release process (no version-bump commit). Run from the repository root.
set -euo pipefail
tag="${TAG:?TAG must be set}"
v="${tag#v}"
if ! [[ "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "::error::tag '$tag' is not vX.Y.Z (optionally vX.Y.Z-pre)"
  exit 1
fi
# The first `version = "…"` in Cargo.toml is [workspace.package]; every crate and the
# Tauri app inherit it. (perl, not sed: macOS sed has no 0,/re/ address.)
perl -0pi -e 's/^version = "[^"]*"/version = "'"$v"'"/m' Cargo.toml
grep -m1 '^version = ' Cargo.toml
# Keep Cargo.lock consistent so --locked builds still work.
cargo update --workspace --quiet
echo "VERSION=$v" >> "${GITHUB_ENV:-/dev/null}"
