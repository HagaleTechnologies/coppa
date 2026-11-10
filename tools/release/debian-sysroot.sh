#!/usr/bin/env bash
# Fetch pinned Debian 11 (glibc 2.31) packages into a sysroot (COP-47).
#
# Usage: tools/release/debian-sysroot.sh <amd64|arm64|armhf> DEST [--runtime]
#
# Without --runtime: the link sysroot for alsa-sys (libasound2 and
# libasound2-dev: alsa.pc and libasound.so). With --runtime: the root the smoke
# test runs the binaries in (libasound2 and libc6), through qemu-user -L or the
# 2.31 loader.
#
# Each package is tried on several mirrors (snapshot.debian.org answers 503
# under load) and trusted only if its sha256 matches the pin below. No root
# needed: packages are unpacked with dpkg-deb -x. To bump a package, change its
# version and sha256 together (docs/RELEASING.md).
set -euo pipefail

usage() {
  echo "usage: $0 <amd64|arm64|armhf> DEST [--runtime]" >&2
  exit 64
}
[ "$#" -ge 2 ] && [ "$#" -le 3 ] || usage
arch=$1
dest=$2
runtime=false
if [ "$#" -eq 3 ]; then
  [ "$3" = "--runtime" ] || usage
  runtime=true
fi

ALSA_VERSION=1.2.4-1.1
GLIBC_VERSION=2.31-13+deb11u11

sha256_for() { # package file name
  case $1 in
    libasound2_${ALSA_VERSION}_amd64.deb)     echo d8c9b5182768db2a7c5c73f1eed0b1be1431ae4f41084d502b325d06d5b0f648 ;;
    libasound2-dev_${ALSA_VERSION}_amd64.deb) echo 73d8b3fa8d161c7a9f3dcb38718925eaf8dbd9d94c1d687087b23aa0719f53e5 ;;
    libasound2_${ALSA_VERSION}_arm64.deb)     echo 8fabd77256593c42e2ad1578f938b56d0a5ed19aa61c6c72c890fd761fd129a8 ;;
    libasound2-dev_${ALSA_VERSION}_arm64.deb) echo 5acd1f3cd1aa11aeeb9dcd2467e8142ced0bdf112b813d134e63baebf342ee29 ;;
    libasound2_${ALSA_VERSION}_armhf.deb)     echo 2b18bdd309f0f127c2ab4829028c7ad89ac27bb3bf42cfc118a5701225131528 ;;
    libasound2-dev_${ALSA_VERSION}_armhf.deb) echo 25e6feb216cc54bc1b9634604402ae80ea040f4e8dc8a92e314d9dc1c5a6062a ;;
    libc6_${GLIBC_VERSION}_amd64.deb)         echo 05f7264da867b37f4c5ce49266b558ea1e81e05a9464f623152fca70f3550282 ;;
    libc6_${GLIBC_VERSION}_arm64.deb)         echo baaa9aa184e2f21738c5819055e6740cc5b22f198e3f416e33f82b40ff6933d8 ;;
    libc6_${GLIBC_VERSION}_armhf.deb)         echo 2b8a57429fd1999ae15c144a09a37a2c37a02ec1c5a5e07eded65455934e3b08 ;;
    *) echo "error: no sha256 pinned for $1" >&2; exit 1 ;;
  esac
}

case $arch in
  amd64|arm64|armhf) ;;
  *) usage ;;
esac

if $runtime; then
  packages=("a/alsa-lib/libasound2_${ALSA_VERSION}_${arch}.deb" "g/glibc/libc6_${GLIBC_VERSION}_${arch}.deb")
else
  packages=("a/alsa-lib/libasound2_${ALSA_VERSION}_${arch}.deb" "a/alsa-lib/libasound2-dev_${ALSA_VERSION}_${arch}.deb")
fi

mkdir -p "$dest"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

for pkg in "${packages[@]}"; do
  file=${pkg##*/}
  sum=$(sha256_for "$file")
  mirrors=(
    "https://deb.debian.org/debian/pool/main/$pkg"
    "https://archive.debian.org/debian/pool/main/$pkg"
  )
  case $pkg in
    a/alsa-lib/*) mirrors+=("https://snapshot.debian.org/archive/debian/20210104T204018Z/pool/main/$pkg") ;;
  esac
  ok=false
  for url in "${mirrors[@]}"; do
    rm -f "$tmp/$file"
    if curl --proto '=https' --tlsv1.2 -fsSL --retry 2 --retry-delay 5 -o "$tmp/$file" "$url" \
      && echo "$sum  $tmp/$file" | sha256sum -c --quiet - 2>/dev/null; then
      ok=true
      break
    fi
    echo "warning: $url failed or did not match the pinned sha256; trying the next mirror" >&2
  done
  if ! $ok; then
    echo "error: could not fetch $file with sha256 $sum from any mirror" >&2
    exit 1
  fi
  dpkg-deb -x "$tmp/$file" "$dest"
  echo "unpacked $file into $dest" >&2
done
