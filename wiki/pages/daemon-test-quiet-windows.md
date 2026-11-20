---
id: daemon-test-quiet-windows
title: What will bite you when a daemon test waits on a quiet window?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/src/event_loop.rs
verified:
  commit: 701a6e4
  date: 2026-11-19
links:
  - daemon-test-ports
  - paused-clock-timers
---
A quiet-window reader ends on the first gap of its configured length, even
when the answer has not arrived. A shared runner stall can therefore make a
positive assertion fail without a modem failure. COP-219 saw this in macOS
Platform Check run 43866152450: the runner stalled after `OK` and before the
`PTT ON` / `PTT OFF` telemetry, so the 1500 ms window returned an empty PTT
list. A rerun on the same commit passed.

Wait for the signal itself in positive assertions. The connecting-state test
now uses `await_ptt_bracket`, which collects until the ordered PTT bracket,
EOF or a 15 s Tokio-time deadline. Keep quiet windows for negative
observations, such as checking that host data while connecting does not
transmit. A stall beyond the finite deadline can still fail, and other
`read_line` callers retain their 2 s timeout exposure.

The helper's duplex tests use `start_paused` to create a deterministic 1600 ms
gap. The daemon test itself uses a live clock: as
[paused-clock-timers](paused-clock-timers.md) explains, Tokio's paused clock
and the daemon's `clock_now()` timers do not advance together.
