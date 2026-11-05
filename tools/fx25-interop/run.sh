#!/bin/sh
# Usage: run.sh /path/to/pinned/direwolf/source
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
source_root=${1:?usage: run.sh DIREWOLF_SOURCE}
sha=$(git -C "$source_root" rev-parse HEAD)
[ "$sha" = de293a1f2526ec6639fe31fa411bd4f2319ecdf4 ] || { echo 'Direwolf 1.7 pinned commit required' >&2; exit 1; }
atest="$source_root/build/src/atest"
[ -x "$atest" ] || { echo "missing pinned Direwolf atest: $atest" >&2; exit 1; }
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
cd "$root"
cargo run --quiet -p coppa-bench --example fx25_wav -- "$scratch/coppa.wav"
"$atest" -B 1200 -F 0 -d x -h -L 1 -G 1 "$scratch/coppa.wav" >"$scratch/atest.txt" 2>&1
cat "$scratch/atest.txt"
python3 tools/fx25-interop/check.py "$scratch/atest.txt"
python3 -m unittest discover -s tools/fx25-interop -p 'test_*.py'
