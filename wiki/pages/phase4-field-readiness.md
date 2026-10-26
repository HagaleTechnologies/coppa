---
id: phase4-field-readiness
title: What will bite you in the daemon's real-radio TX/RX path?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/src/**
  - crates/coppa-radio/src/**
  - crates/coppa-ffi/src/lib.rs
  - docs/OPERATING.md
  - docs/SPEC.md
verified:
  commit: 59b0b63
  date: 2026-07-14
links:
  - overview
  - coppa-engine
  - coppa-protocol
  - cpal-feature-gate
---
Phase 4 made the daemon field-ready: real serial (DTR/RTS) and Linux GPIO PTT,
a busy-channel gate, station-ID timer and beacon mode, a two-tone TUNE command,
a WebSocket spectrum stream, live `coppa rx`, an FFI v2 binary API, and the
normative `docs/SPEC.md`. The invariants below were each established by fixing
a real bug — breaking them will reintroduce that bug.

## Invariants to preserve

- **Host data keys TX only inside an established session (COP-29).** The
  `DataReceived` arm drops data-port/WebSocket bytes when no session is
  established, unless `[host] sessionless_tx_enabled` is set. The key defaults
  to off; a `Connecting` session does not count. The sessionless raw/ARQ path
  behind that key is the daemon's only path for ARQ-tracked host data
  (RateLoop/probing feedback), so don't delete it as dead code.
- **Replies wait out the peer's PTT tail (COP-60).** `transmit_samples` holds
  any key-up until `[radio] turnaround_ms` after the last decoded frame and
  drains the input ring while it waits, since a plain sleep overflowed the
  8,192-sample ring. Don't move the wait out of the chokepoint or replace the
  drain with a sleep. See `docs/OPERATING.md` and
  `docs/testing/cable-loop-turnaround.md`.
- **Every transmission goes through `transmit_samples`** — it is the single
  chokepoint applying PTT lead/tail, busy-defer, and station-ID accounting.
  ARQ retransmits and session keepalives once bypassed it via a direct
  `handle_audio_out` call; harmless with stub PTT, but with real PTT they would
  silently never key the radio. Any new TX call site must route through it.
- **TX audio is paced, never dropped, and PTT follows playback (COP-22).**
  Frames are larger than every ring on the path (65,520-sample OFDM frame vs.
  the 8,192-sample audio-out and `CpalSink` rings; 94k-110k-sample AFSK frames
  vs. the TNC's 65,536), so each producer retains its unwritten suffix
  (`audio_output::PendingOutput`, `OutputWorker`, the CLI's `play_keyed`) —
  a one-shot `write` once silently dropped ~87% of every frame. PTT is
  released from `CpalSink::playback_progress` (consumed count plus CPAL's
  predicted presentation time), not from a nominal duration, and only by the
  newest transmission's generation-tagged `TxComplete`. Output failure, 5 s
  without playback progress, or `max_tx_duration_s` cancels and unkeys; no
  hardware/on-air test has validated the timing yet.
- **PTT is released on every exit path (COP-31).** Four layers: backend
  `Drop` (rigctld, serial, GPIO), an explicit release at shutdown and at
  startup, an in-loop backstop on the 500 ms tick, and an out-of-process
  `coppad --ptt-guardian` for rigctld/GPIO that releases after a SIGKILL or
  crash. `EventLoop::set_ptt` is the only place the guardian is told about PTT
  state, so any new direct `self.ptt.set_ptt` call must report to it too, or
  the guardian will miss a key or fire a duplicate release. Exit-path table
  and residuals: `docs/OPERATING.md`, "PTT safety when coppad stops".
- **Every ARQ retransmission must call `ArqTx::mark_retransmitted`.** Before
  this was wired, `get_retransmits`'s expiry check never advanced: every
  unacked segment retransmitted every 500 ms forever, and the `max_retransmit`
  give-up cap never fired (pre-existing since Phase 3, found at Phase 4's final
  review).
- **Decoded payloads are raw bytes end-to-end.** The streaming decode path once
  forced UTF-8 on every payload, silently dropping all binary/compressed MAC
  PDUs — i.e. all real over-the-air session/ARQ traffic (the Phase 3 benches
  never caught it because they drive `ArqTx`/`ArqRx` directly, bypassing the
  audio path). See [[coppa-engine]].
- **The busy-wait loop must not run full protocol dispatch.** While deferring
  TX on a busy channel, the daemon pumps audio only into `busy_gate.observe()`
  (accumulating traffic for later dispatch); feeding full dispatch from inside
  the wait once allowed a decoded CONNECT_REQ to trigger a nested transmit,
  bypassing the single-in-flight guard.
- **One TCP read is not one over-the-air frame (COP-41).** Every
  `DataReceived` is chunked to pieces that encode at their target level
  (session/raw: current level; ARQ: the ladder-minimum budget, so segments
  survive rate drops); the ARQ window's overflow waits in a backlog counted by
  `BUFFER`. Don't re-introduce a whole-event send on any TX path.

## Configuration gotchas

- PTT config uses extended syntax: `serial:/dev/ttyUSB0:dtr`, `gpio:17`, or
  the flat `none`/`vox`/`rigctld`. Unrecognized values hard-error at startup —
  bare `"serial"` is NOT valid. `rigctld` unreachable at startup still falls
  back to NullPtt, but since COP-32 it logs at ERROR and the daemon logs
  `Daemon running degraded` instead of `Daemon ready`. There is still no
  reconnect, so PTT stays lost for that run.
- Busy-defer is deliberately independent of callsign (channel courtesy ≠
  identification); station-ID and beacon do gate on callsign.
- Hardware audio still needs `cpal-backend` (see [[cpal-feature-gate]]), and
  the daemon's spectrum module needs `websocket`.

## CI gotcha for contributors

CI's Clippy job runs `cargo clippy --workspace --all-targets -- -D warnings`
with NO feature flags; feature-gated code referenced only from gated call
sites will fail it as dead code. Verify clippy both bare and with
`--features cpal-backend,websocket` before pushing.
