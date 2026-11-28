---
id: daemon-test-quiet-windows
title: What will bite you when a daemon test waits on a quiet window?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/src/event_loop.rs
verified:
  commit: d2bcc0a
  date: 2026-11-23
links:
  - daemon-test-ports
  - paused-clock-timers
---
A quiet-window reader ends on the first gap of its configured length, even
when the answer has not arrived. A shared runner stall can therefore make a
positive assertion fail without a modem failure. COP-219 saw this in macOS
Platform Check run 43866152450: the runner stalled after `OK` and before the
`PTT ON` / `PTT OFF` telemetry, so the 1500 ms window returned an empty PTT
list. A rerun on the same commit passed. COP-232 reported the same failure
from PR #283's run 44631940209, on a branch cut before COP-219 merged.

Wait for the signal itself in positive assertions. The connecting-state test
now uses `await_ptt_bracket`, which collects until the ordered PTT bracket,
EOF or a 15 s Tokio-time deadline. Keep quiet windows for negative
observations, such as checking that host data while connecting does not
transmit. Other `read_line` callers on a live clock keep their 2 s timeout
exposure.

Since COP-216, both host-data tests run under `start_paused`, as do the
helper's duplex tests, which use it for a deterministic 1600 ms gap. The
daemon reads its timers through `clock_now()`, which is Tokio's clock, so the
15 s deadline, the 1500 ms quiet windows and the connect retry all move in
virtual time. Tokio advances a paused clock only when the runtime has nothing
to run, so a runner stall does not use up the deadline or close a window
early. [paused-clock-timers](paused-clock-timers.md) explains why the quiet
window ends before the connect retry fires.
