#!/usr/bin/env bash
# The release tag (v1.2.3) must match the workspace version and the app's version.
set -euo pipefail
tag="${1#v}"
cargo=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml)
tauri=$(sed -n 's/^ *"version": *"\(.*\)",*/\1/p' crates/timber-desktop/tauri.conf.json | head -1)
echo "tag $tag · Cargo.toml $cargo · tauri.conf.json $tauri"
if [ "$tag" != "$cargo" ] || [ "$tag" != "$tauri" ]; then
  echo "::error::Tag v$tag doesn't match the versions in Cargo.toml ($cargo) and tauri.conf.json ($tauri)"
  exit 1
fi
