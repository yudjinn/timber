#!/usr/bin/env bash
# Sparse, shallow kernel checkout with just what Timber's tests read: the device trees of
# arm64, arm and riscv, the dt-bindings and uapi headers, and the dts Makefiles.
# Usage: fetch-kernel.sh TAG DIR   (about 100 MB, half a minute)
set -euo pipefail
tag="$1"
dir="$2"
if [ -f "$dir/.timber-kernel-$tag" ]; then
  echo "kernel $tag already in $dir"
  exit 0
fi
rm -rf "$dir"
git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$tag" --filter=blob:none --sparse \
  https://github.com/torvalds/linux.git "$dir"
git -C "$dir" sparse-checkout set --no-cone \
  /Makefile /scripts/dtc/Makefile \
  /arch/arm64/boot/dts/ /arch/arm/boot/dts/ /arch/riscv/boot/dts/ \
  /include/dt-bindings/ /include/uapi/
touch "$dir/.timber-kernel-$tag"
echo "kernel $tag in $dir ($(du -sh "$dir" | cut -f1))"
