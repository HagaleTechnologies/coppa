#!/usr/bin/env bash
# Check and run Linux release binaries (COP-47).
#
# Usage:
#   tools/release/smoke-linux.sh inspect TARGET BINDIR
#   tools/release/smoke-linux.sh run TARGET BINDIR RUNTIME_ROOT VERSION
#
# inspect: coppa and coppad are built for TARGET, need no glibc symbol newer
#   than 2.31, link only the C library family and libasound.so.2, and do not
#   link libudev. A native build on Ubuntu 24.04 fails here (GLIBC_2.39): that
#   is the defect this check exists to stop.
# run: runs both binaries against the Debian 11 root from
#   `debian-sysroot.sh <arch> RUNTIME_ROOT --runtime` (glibc 2.31, ALSA 1.2.4):
#   x86-64 through that root's ld-2.31.so, ARM through qemu-user -L. qemu comes
#   from PATH or from $QEMU_DIR.
set -euo pipefail

GLIBC_FLOOR=2.31
ALLOWED_NEEDED=(
  libasound.so.2 libc.so.6 libm.so.6 libpthread.so.0 libdl.so.2 librt.so.1 libgcc_s.so.1
  ld-linux-x86-64.so.2 ld-linux-aarch64.so.1 ld-linux-armhf.so.3
)
CLI_FEATURES="features: cpal-backend,file-backend,kiss-tnc"
DAEMON_FEATURES="features=ardop,cpal-backend,gpio-ptt,kiss-tnc,mdns,serial-ptt,websocket"

die() {
  echo "::error::$*" >&2
  exit 1
}

usage() {
  echo "usage: $0 inspect TARGET BINDIR" >&2
  echo "       $0 run TARGET BINDIR RUNTIME_ROOT VERSION" >&2
  exit 64
}

machine_for() {
  case $1 in
    x86_64-unknown-linux-gnu) echo "Advanced Micro Devices X86-64" ;;
    aarch64-unknown-linux-gnu) echo "AArch64" ;;
    armv7-unknown-linux-gnueabihf) echo "ARM" ;;
    *) die "unknown Linux release target $1" ;;
  esac
}

inspect() {
  local target=$1 bindir=$2 machine b f got highest needed lib ok allowed
  machine=$(machine_for "$target")
  for b in coppa coppad; do
    f="$bindir/$b"
    [ -f "$f" ] || die "$f is missing"
    got=$(readelf -h "$f" | sed -n 's/^ *Machine: *//p')
    [ "$got" = "$machine" ] || die "$f is built for '$got', expected '$machine' ($target)"
    highest=$(readelf -V "$f" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)
    [ -n "$highest" ] || die "$f has no GLIBC symbol versions; is it a glibc build?"
    if [ "$(printf '%s\n%s\n' "$highest" "$GLIBC_FLOOR" | sort -V | tail -n 1)" != "$GLIBC_FLOOR" ]; then
      die "$f needs GLIBC_$highest, above the $GLIBC_FLOOR floor; build with cargo zigbuild --target $target.$GLIBC_FLOOR"
    fi
    needed=$(readelf -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\].*/\1/p')
    # Checked by name first so the message says why (serialport's libudev
    # feature must stay off), not just "not in the allowed set".
    if grep -qx 'libudev.so.1' <<<"$needed"; then
      die "$f links libudev.so.1; keep serialport at default-features = false"
    fi
    for lib in $needed; do
      ok=false
      for allowed in "${ALLOWED_NEEDED[@]}"; do
        if [ "$lib" = "$allowed" ]; then ok=true; fi
      done
      $ok || die "$f links $lib, which is not in the allowed set: ${ALLOWED_NEEDED[*]}"
    done
    echo "$f: $got, highest GLIBC_$highest, NEEDED: $(echo $needed)"
  done
}

find_qemu() {
  local name=$1
  if [ -n "${QEMU_DIR:-}" ] && [ -x "$QEMU_DIR/$name" ]; then
    echo "$QEMU_DIR/$name"
  elif command -v "$name" >/dev/null 2>&1; then
    command -v "$name"
  else
    die "$name not found on PATH or in QEMU_DIR"
  fi
}

runner=()
set_runner() {
  local target=$1 root=$2 loader
  case $target in
    x86_64-unknown-linux-gnu)
      loader="$root/lib/x86_64-linux-gnu/ld-2.31.so"
      [ -x "$loader" ] || die "$loader is missing; run debian-sysroot.sh amd64 $root --runtime"
      runner=("$loader" --library-path "$root/lib/x86_64-linux-gnu:$root/usr/lib/x86_64-linux-gnu")
      ;;
    aarch64-unknown-linux-gnu) runner=("$(find_qemu qemu-aarch64-static)" -L "$root") ;;
    armv7-unknown-linux-gnueabihf) runner=("$(find_qemu qemu-arm-static)" -L "$root") ;;
    *) die "unknown Linux release target $target" ;;
  esac
}

run() {
  local target=$1 bindir=$2 root=$3 version=$4 out line1 line3 first
  # Absolute paths: the loopback and the daemon run from a scratch directory.
  root=$(cd "$root" && pwd)
  bindir=$(cd "$bindir" && pwd)
  set_runner "$target" "$root"
  tmp=$(mktemp -d)
  # EXIT, not RETURN: die() exits, and the scratch directory goes either way.
  trap 'rm -rf "$tmp"' EXIT

  out=$("${runner[@]}" "$bindir/coppa" --version)
  echo "$out"
  line1=$(sed -n 1p <<<"$out")
  line3=$(sed -n 3p <<<"$out")
  [ "$line1" = "coppa $version" ] || die "coppa --version line 1 is '$line1', expected 'coppa $version'"
  [ "$line3" = "$CLI_FEATURES" ] || die "coppa --version line 3 is '$line3', expected '$CLI_FEATURES'"

  out=$(cd "$tmp" && "${runner[@]}" "$bindir/coppa" loopback "Hello from Coppa" 2>&1)
  echo "$out"
  grep -qF 'PASS: Loopback test successful!' <<<"$out" || die "coppa loopback did not pass"

  out=$("${runner[@]}" "$bindir/coppad" --version)
  echo "$out"
  grep -q "^coppad $version\$" <<<"$(sed -n 1p <<<"$out")" || die "coppad --version prints '$out', expected 'coppad $version'"

  # The daemon's first log line is its build banner. With an empty config and
  # no sound card it may exit with an error after that line; only the banner
  # is checked.
  : > "$tmp/empty.toml"
  (cd "$tmp" && NO_COLOR=1 timeout 30 "${runner[@]}" "$bindir/coppad" "$tmp/empty.toml" > "$tmp/coppad.log" 2>&1) || true
  first=$(sed -n 1p "$tmp/coppad.log")
  echo "$first"
  grep -qF "version=\"$version\"" <<<"$first" || die "coppad banner lacks version=\"$version\": $first"
  grep -qF "$DAEMON_FEATURES" <<<"$first" || die "coppad banner lacks $DAEMON_FEATURES: $first"
  grep -q 'git_sha="' <<<"$first" || die "coppad banner has no git_sha: $first"
  if grep -qF 'git_sha="unknown"' <<<"$first"; then
    die "coppad banner has git_sha=\"unknown\"; set COPPA_GIT_SHA for the build"
  fi
  echo "$target: smoke run passed"
}

[ "$#" -ge 1 ] || usage
mode=$1
shift
case $mode in
  inspect) [ "$#" -eq 2 ] || usage; inspect "$@" ;;
  run) [ "$#" -eq 4 ] || usage; run "$@" ;;
  *) usage ;;
esac
