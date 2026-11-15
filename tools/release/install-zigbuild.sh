#!/usr/bin/env bash
# Install pinned zig and cargo-zigbuild into DEST (COP-47).
#
# Usage: tools/release/install-zigbuild.sh DEST
#
# Prints the two directories to add to PATH, one per line. Every download is
# checked against the sha256 pinned below; a mismatch stops the script. To bump
# a version, change the URL and its sha256 together (docs/RELEASING.md).
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 DEST" >&2
  exit 64
fi
dest=$1
mkdir -p "$dest"
dest=$(cd "$dest" && pwd)

ZIG_VERSION=0.13.0
ZIG_URL="https://ziglang.org/download/${ZIG_VERSION}/zig-linux-x86_64-${ZIG_VERSION}.tar.xz"
ZIG_SHA256=d45312e61ebcc48032b77bc4cf7fd6915c11fa16e4aad116b66c9468211230ea

ZIGBUILD_VERSION=0.24.1
ZIGBUILD_URL="https://github.com/rust-cross/cargo-zigbuild/releases/download/v${ZIGBUILD_VERSION}/cargo-zigbuild-x86_64-unknown-linux-musl.tar.xz"
ZIGBUILD_SHA256=f09b180a517af0e4dd4bffc921bb88e5ab705452a9dd948c61e489b3020c68c2

fetch() { # url sha256 out
  local url=$1 sum=$2 out=$3
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --retry-delay 5 -o "$out" "$url"
  if ! echo "$sum  $out" | sha256sum -c --quiet -; then
    echo "error: sha256 mismatch for $url" >&2
    exit 1
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch "$ZIG_URL" "$ZIG_SHA256" "$tmp/zig.tar.xz"
tar -xJf "$tmp/zig.tar.xz" -C "$dest"
zig_dir="$dest/zig-linux-x86_64-${ZIG_VERSION}"

fetch "$ZIGBUILD_URL" "$ZIGBUILD_SHA256" "$tmp/zigbuild.tar.xz"
tar -xJf "$tmp/zigbuild.tar.xz" -C "$dest"
zigbuild_dir="$dest/cargo-zigbuild-x86_64-unknown-linux-musl"
# Older release tarballs put the binary at the top level.
if [ ! -x "$zigbuild_dir/cargo-zigbuild" ] && [ -x "$dest/cargo-zigbuild" ]; then
  zigbuild_dir=$dest
fi

"$zig_dir/zig" version >&2
"$zigbuild_dir/cargo-zigbuild" --version >&2

echo "$zig_dir"
echo "$zigbuild_dir"
