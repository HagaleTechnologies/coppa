#!/usr/bin/env bash
# tools/tnc-bench/run.sh: coppa's AFSK 1200 TNC against Direwolf on track 2 of
# the WA8LMF TNC Test CD (COP-112).
#
# Fetches TNC_Test_2.iso once into a cache dir, checks it and the extracted
# track against pinned SHA-256 sums, decodes the FLAC to WAV, and runs
# coppa-bench's afsk_vs_direwolf diagnostic, which counts the packets coppa's
# demodulator, `direwolf` and `atest` decode and prints the report on stdout.
# No audio is committed to the repo. See README.md next to this file.
set -euo pipefail

ISO_URL=http://wa8lmf.net/TNCtest/TNC_Test_2.iso
ISO_SHA256=28a115891ba94aa9df9fe00abfe17dde190804f2013647c118b38225fc69e09c
TRACK=Audio-Tracks/02_100-Mic-E-Bursts-DE-emphasized.flac # CD track 2 despite the name
FLAC_SHA256=534323df93cbb9ad52febbfed830dd473f9e7a01cef891b93205ee2264ed6d45
WAV_SHA256=006e35196b05ce6e22c9e824111c49dd7928e80e29749f9fad1787069c8ae495 # flac 1.4.3

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)

usage() {
    cat <<'EOF'
Usage: tools/tnc-bench/run.sh [options]

Fetches track 2 of the WA8LMF TNC Test CD (the 25:50 de-emphasized recording
of 144.39 MHz APRS traffic), checks it against pinned SHA-256 sums, and counts
the AX.25 packets that coppa's AFSK 1200 demodulator and Direwolf decode from it.

Options:
  --iso PATH       use this TNC_Test_2.iso instead of the cached download
  --direwolf PATH  the direwolf program (default: direwolf on PATH)
  --atest PATH     Direwolf's atest program (default: atest on PATH)
  --keep           keep the scratch directory (WAV, direwolf.conf, both programs' output)
  -h, --help       show this help

Needs curl (first run only), python3, flac, sha256sum or shasum, cargo,
direwolf and atest. Do not play the track over the air.
Exit status: 0 report printed, 1 a decoder failed, 2 setup error.
EOF
}

say() { echo "tnc-bench: $*" >&2; }
setup_fail() {
    say "$*"
    exit 2
}

ISO=
DIREWOLF=direwolf
ATEST=atest
KEEP=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --iso)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            ISO=$2
            shift 2
            ;;
        --direwolf)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            DIREWOLF=$2
            shift 2
            ;;
        --atest)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            ATEST=$2
            shift 2
            ;;
        --keep)
            KEEP=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

need() {
    command -v "$1" >/dev/null 2>&1 || setup_fail "missing tool: $1 ($2)"
}

need python3 "Debian/Ubuntu: apt install python3"
need flac "Debian/Ubuntu: apt install flac"
need cargo "https://rustup.rs"
if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    setup_fail "missing tool: sha256sum (Debian/Ubuntu: apt install coreutils; macOS: shasum ships with the OS)"
fi
command -v "$DIREWOLF" >/dev/null 2>&1 ||
    setup_fail "missing tool: $DIREWOLF (Debian/Ubuntu: apt install direwolf, or pass --direwolf PATH)"
command -v "$ATEST" >/dev/null 2>&1 ||
    setup_fail "missing tool: $ATEST (Debian/Ubuntu: apt install direwolf, or pass --atest PATH)"
DIREWOLF=$(command -v "$DIREWOLF")
ATEST=$(command -v "$ATEST")

# The ISO: --iso, or the cached download.
CACHED=0
if [[ -z "$ISO" ]]; then
    CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/coppa-tnc-bench"
    ISO="$CACHE_DIR/TNC_Test_2.iso"
    CACHED=1
    if [[ ! -f "$ISO" ]]; then
        need curl "Debian/Ubuntu: apt install curl"
        mkdir -p "$CACHE_DIR"
        say "downloading $ISO_URL to $CACHE_DIR/"
        curl -fL --retry 3 -sS -o "$ISO.part" "$ISO_URL" || setup_fail "download failed: $ISO_URL"
        mv "$ISO.part" "$ISO"
    fi
fi
[[ -f "$ISO" ]] || setup_fail "$ISO: no such file"

check_pin() { # path want label; exits 2 on a mismatch
    local got
    got=$(sha256 "$1")
    if [[ "$got" != "$2" ]]; then
        say "$3 sha256 mismatch"
        echo "  got  $got" >&2
        echo "  want $2" >&2
        return 1
    fi
}

if ! check_pin "$ISO" "$ISO_SHA256" "$ISO"; then
    [[ $CACHED -eq 1 ]] && say "delete $ISO to download it again"
    exit 2
fi
say "$(basename "$ISO") sha256 OK"

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/tnc-bench.XXXXXX")
cleanup() {
    if [[ $KEEP -eq 1 ]]; then
        say "kept $SCRATCH"
    else
        rm -rf "$SCRATCH"
    fi
}
trap cleanup EXIT

python3 -I "$HERE/iso_extract.py" "$ISO" "$TRACK" "$SCRATCH/track2.flac" ||
    setup_fail "could not extract $TRACK from $ISO"
check_pin "$SCRATCH/track2.flac" "$FLAC_SHA256" "$TRACK" || exit 2
say "track 2 is $TRACK, sha256 OK"

flac -d -s -f -o "$SCRATCH/track2.wav" "$SCRATCH/track2.flac" ||
    setup_fail "flac could not decode $SCRATCH/track2.flac"
FLAC_VERSION=$(flac --version 2>/dev/null | head -n 1)
WAV_GOT=$(sha256 "$SCRATCH/track2.wav")
if [[ "$WAV_GOT" == "$WAV_SHA256" ]]; then
    say "$FLAC_VERSION decoded track2.wav, sha256 OK"
else
    say "$FLAC_VERSION decoded track2.wav"
    say "note: track2.wav sha256 $WAV_GOT differs from the pin $WAV_SHA256; the audio is fixed by the FLAC sha256"
fi

COPPA_SHA=$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)
if [[ -n "$(git -C "$REPO" status --porcelain --untracked-files=no 2>/dev/null)" ]]; then
    TREE="uncommitted changes"
else
    TREE="tree clean"
fi
say "coppa $COPPA_SHA ($TREE), direwolf $DIREWOLF, atest $ATEST"

say "building and running coppa-bench --example afsk_vs_direwolf (release)"
cd "$REPO"
status=0
cargo run --release -q -p coppa-bench --example afsk_vs_direwolf -- \
    --wav "$SCRATCH/track2.wav" --direwolf "$DIREWOLF" --atest "$ATEST" \
    --log-dir "$SCRATCH" || status=$?
if [[ $status -ne 0 ]]; then
    exit 1
fi
