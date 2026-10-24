#!/usr/bin/env bash
# The release tag (v1.2.3) must match the workspace version in Cargo.toml, the one place the
# version is set: tauri.conf.json leaves it out, so the app takes it from Cargo.toml too.
set -euo pipefail
tag="${1#v}"
cargo=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml)
echo "tag $tag · Cargo.toml $cargo"
if grep -q '^  "version"' crates/timber-desktop/tauri.conf.json; then
  echo "::error::tauri.conf.json sets its own version; remove it so the app uses Cargo.toml's"
  exit 1
fi
if [ "$tag" != "$cargo" ]; then
  echo "::error::Tag v$tag doesn't match the version in Cargo.toml ($cargo)"
  exit 1
fi
