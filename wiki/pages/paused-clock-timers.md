---
id: paused-clock-timers
title: What will bite you when a paused-clock test drives the daemon's timers?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/src/event_loop.rs
  - crates/coppa-daemon/src/audio_output.rs
  - crates/coppa-daemon/clippy.toml
verified:
  commit: 2526b50
  date: 2026-11-04
links:
  - overview
---
Under `start_paused`, only tokio's clock is virtual. DSP compute burns real
time. A session timer on the std clock therefore races a test's virtual
waits, so CPU load can decide whether an ACK arrives before a retransmit.
COP-142 reproduced this under load after about one failure in 100 CI runs.

The event loop reads time through `clock_now()`. The crate-local
`clippy.toml` enforces that rule in production event-loop code. Its one
deliberate std read is the `PlayoutWatch` base, because `wait_for_playout`
already maps tokio elapsed time onto that base. Converting the base too
would count virtual time twice.

In paused tests, move timers with `tokio::time::advance`, never
`std::thread::sleep`. On an unpaused runtime the clocks agree.

To expose a load-dependent failure, build once, copy the binary so a later
build cannot replace it, and oversubscribe the cgroup CPU quota eightfold.
Require every copy of each test to pass. Use a session-private scratch
directory in place of the example's `/tmp/cop142-*` paths when sharing a host.

```bash
cargo test -p coppa-daemon --lib --features ardop --no-run 2>&1 | grep Executable
BIN=target/debug/deps/coppa_daemon-<hash>          # path from the line above
cp "$BIN" /tmp/cop142-bin                          # a later build must not replace it mid-run
Q=$(awk '{print ($1=="max") ? 2 : int($1/$2)}' /sys/fs/cgroup/cpu.max); COPIES=$((8*Q))
for T in ardop_session_data_flows_and_buffer_falls_on_peer_ack \
         ardop_dial_maps_onto_the_connect_path \
         ardop_disconnect_tears_down_and_newstate_follows; do
  NAME="event_loop::tests::ardop_host::$T"; OUT=/tmp/cop142-$T; mkdir -p "$OUT"
  for i in $(seq 1 "$COPIES"); do
    ( /tmp/cop142-bin --exact "$NAME" > "$OUT/$i.log" 2>&1; echo "exit=$?" >> "$OUT/$i.log" ) &
  done; wait
  echo "$T: $(grep -l '^exit=0' "$OUT"/*.log | wc -l) of $COPIES passed"
done
```

The quota calculation falls back to two CPUs for an unlimited cgroup.
On an unthrottled host, set `COPIES` to eight times `nproc` instead.
