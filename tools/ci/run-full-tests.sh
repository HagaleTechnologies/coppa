#!/usr/bin/env bash
# Keep target selection explicit: --all-targets also selects benchmarks.
set -euo pipefail
export RUST_TEST_THREADS=${RUST_TEST_THREADS:-2}
features=cpal-backend,websocket,ardop,mdns,kiss-tnc
timed() {
  local label=$1 start=$SECONDS rc=0
  shift
  "$@" || rc=$?
  printf 'full-ci group=%s phase=%s seconds=%s exit=%s\n' "$group" "$label" "$((SECONDS-start))" "$rc"
  return "$rc"
}
selection() {
  timed compile cargo test --locked --profile ci "$@" --no-run
  timed execute cargo test --locked --profile ci "$@"
}
docs() {
  timed doctest cargo test --locked --profile ci "$@" --doc
}
group=${1:-}
case "$group" in
  workspace)
    selection --workspace --exclude coppa-daemon --features cpal-backend,websocket,ardop,kiss-tnc --lib --bins --tests
    timed compile-examples cargo test --locked --profile ci --workspace --exclude coppa-daemon --features cpal-backend,websocket,ardop,kiss-tnc --examples --no-run
    docs --workspace --exclude coppa-daemon --features cpal-backend,websocket,ardop,kiss-tnc
    ;;
  daemon-unit)
    selection -p coppa-daemon --features "$features" --lib --bins
    ;;
  daemon-integration)
    selection -p coppa-daemon --features "$features" --test '*'
    docs -p coppa-daemon --features "$features"
    timed compile-examples cargo test --locked --profile ci -p coppa-daemon --features "$features" --examples --no-run
    ;;
  no-audio)
    selection -p coppa-cli --features kiss-tnc --test tnc_agw
    selection -p coppa-daemon --features websocket,mdns,kiss-tnc --test cli --test ptt_release --test reload --test runtime_smoke
    ;;
  *) echo "Usage: $0 {workspace|daemon-unit|daemon-integration|no-audio}" >&2; exit 2 ;;
esac
