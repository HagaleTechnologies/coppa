---
id: session-fade-protection
title: What will bite you about session fade protection?
kind: gotcha
status: current
maintainer: agent
sources:
  - docs/SPEC.md
  - docs/adr/012-cop119-session-outer-code.md
  - crates/coppa-daemon/src/event_loop.rs
verified:
  commit: d837c89
  date: 2026-11-04
links:
  - coppa-protocol
  - overview
---
Two default `accept` stations do not enable fade protection. One must set
`[session] fade_protection = "request"`; either station's `off` disables it.
See SPEC §16 for the wire rules and ADR-012 for the choices.

Each packet needs its own PHY HARQ key. Sharing key zero lets failed payloads
from unrelated packets combine. Retransmissions reuse the same block keys,
and delivery clears current and previous block keys.

An ACK belongs after the full burst, even if packet six completes decoding.
The receiver also holds its queued host traffic until the burst ends. The
first decoded packet starts that hold, so a wholly undecoded burst remains
vulnerable to a collision. Two ACK copies reduce the ACK-loss weak point.

The TX-duration guard can send a plain frame inside a protected session.
`link.fade_protection` still reports the negotiated mode. `fade_hold` is a
separate retransmission state. See OPERATING for configuration and BENCHMARKS
for the measured simulation limits.
