#!/usr/bin/env bash
# Generate the synthetic COP-116 Direwolf recording and verify its pin.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
PIN=11487f60768c86e6eb841fab197016232f85f0447565806eab2fcff28b7d80e5
FORCE=false
PARITY=false
if command -v direwolf >/dev/null 2>&1; then PARITY=true; fi
for arg in "$@"; do
    case "$arg" in
        --force) FORCE=true ;;
        --parity) PARITY=true ;;
        --no-parity) PARITY=false ;;
        --help) echo "Usage: $0 [--force] [--parity|--no-parity]"; exit 0 ;;
        *) echo "Unknown argument: $arg" >&2; exit 2 ;;
    esac
done
for tool in gen_packets cargo sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || { echo "Missing tool: $tool" >&2; exit 2; }
done
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/coppa-g3ruh.XXXXXX")
trap 'rm -rf "$SCRATCH"' EXIT
cd "$ROOT"
gen_packets -B 9600 -r 48000 -a 50 -o "$SCRATCH/clean.wav" testdata/g3ruh/frames.txt
cargo run --release -q -p coppa-bench --example g3ruh_fixture_gen -- \
    --clean "$SCRATCH/clean.wav" --out "$SCRATCH/noisy.wav"
ACTUAL=$(sha256sum "$SCRATCH/noisy.wav" | cut -d ' ' -f 1)
echo "fixture SHA-256: $ACTUAL (pin: $PIN)"
if [[ "$ACTUAL" != "$PIN" && "$FORCE" != true ]]; then
    echo "Pin mismatch. Inspect the generator before deliberately using --force." >&2
    exit 2
fi
TARGET=testdata/g3ruh/g3ruh_9600_100frames_10db.wav
if [[ -e "$TARGET" && "$FORCE" != true ]]; then
    CURRENT=$(sha256sum "$TARGET" | cut -d ' ' -f 1)
    if [[ "$CURRENT" != "$ACTUAL" ]]; then
        echo "Existing fixture differs; use --force to replace deliberately." >&2
        exit 2
    fi
else
    cp "$SCRATCH/noisy.wav" "$TARGET"
fi
echo 'Reference commands: atest -B 9600 testdata/g3ruh/g3ruh_9600_100frames_10db.wav'
echo 'direwolf -c direwolf.conf -t 0 < channel-0-16bit-pcm.raw (config: MODEM 9600, ARATE 48000)'
if [[ "$PARITY" == true ]]; then
    cargo run --release -q -p coppa-bench --example g3ruh_vs_direwolf -- --wav "$TARGET"
fi
