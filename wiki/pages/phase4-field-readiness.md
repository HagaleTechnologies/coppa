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

- **Every transmission goes through `transmit_samples`** — it is the single
  chokepoint applying PTT lead/tail, busy-defer, and station-ID accounting.
  ARQ retransmits and session keepalives once bypassed it via a direct
  `handle_audio_out` call; harmless with stub PTT, but with real PTT they would
  silently never key the radio. Any new TX call site must route through it.
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

## Configuration gotchas

- PTT config uses extended syntax: `serial:/dev/ttyUSB0:dtr`, `gpio:17`, or
  the flat `none`/`vox`/`rigctld`. Unrecognized values hard-error at startup —
  bare `"serial"` is NOT valid. Known residual gap: `rigctld` with an
  unreachable rig at startup falls back to NullPtt with only a warning
  (a daemon that runs with silently-broken PTT).
- Busy-defer is deliberately independent of callsign (channel courtesy ≠
  identification); station-ID and beacon do gate on callsign.
- Hardware audio still needs `cpal-backend` (see [[cpal-feature-gate]]), and
  the daemon's spectrum module needs `websocket`.

## CI gotcha for contributors

CI's Clippy job runs `cargo clippy --workspace --all-targets -- -D warnings`
with NO feature flags; feature-gated code referenced only from gated call
sites will fail it as dead code. Verify clippy both bare and with
`--features cpal-backend,websocket` before pushing.
