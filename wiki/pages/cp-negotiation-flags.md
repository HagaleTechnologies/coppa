---
id: cp-negotiation-flags
title: Which flags actually turn CP-switch negotiation on, and what does short CP buy?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-daemon/src/config.rs
  - crates/coppa-daemon/src/event_loop.rs
  - crates/coppa-protocol/src/modem/airtime.rs
  - docs/adr/008-phase3-system-layer.md
verified:
  commit: 2efc1c1
  date: 2026-08-05
links:
  - cp-negotiation-gotchas
  - adr-008-phase3-system-layer
---
Two traps live one level up from the handshake itself — in the config flags that
gate it, and in what a CP change can and cannot move on a benchmark. The
handshake's own traps are on [[cp-negotiation-gotchas]].

## 1. Enabling CP negotiation is three flags, not one

`cp_negotiation_enabled` reads like the switch for this whole subsystem. It is
not: live negotiation needs `arq_enabled` **and** `cp_gate_enabled` **and**
`cp_negotiation_enabled`. The field doc in `crates/coppa-daemon/src/config.rs`
spells out what each of the three flags gates, and carries the reverse
cross-reference on `cp_gate_enabled`.

The distinction that matters, and the one prose keeps flattening, is
**responder vs. initiator**:

- The flag alone **never initiates**. The only code that can put a `Propose` on
  the air sits inside `if self.config.engine.cp_gate_enabled` in
  `event_loop.rs`, so with the gate off the propose block is structurally
  unreachable and `CpGate::observe` is never even called.
- The flag alone **does** make the station a full responder once `arq_enabled`
  is on, because `handle_cp_control` gates on `cp_negotiation_enabled` and
  nothing else. A peer can then talk this station onto short CP without it ever
  asking.

So "the flag is inert" holds only while `arq_enabled` is off, and "the flag
turns the feature on" is never true. Both halves are enforced rather than
merely documented — by
`cp_negotiation_enabled_alone_never_initiates_without_cp_gate` (named for what
it proves: never *initiates*) and by
`test_cp_negotiation_requires_two_more_flags_by_default`, the tripwire that
makes a future *partial* flip impossible to land silently. COP-2 evaluated the
flip and declined it precisely because flipping this flag alone changes nothing
an operator could observe.

## 2. Short CP cannot move a ratio

Anyone reaching for short CP to improve a goodput *ratio* should read
`coppa_protocol::modem::airtime`'s
`short_cp_scales_frame_airtime_by_one_constant_at_every_level` first. A uniform
CP change is one constant airtime divisor — the same `1104/1260` at every speed
level — so that airtime factor alone cancels out of `adaptive/best-fixed`,
`adaptive/oracle`, or any other ratio of two goodputs measured under it. It is
a real absolute-goodput lever, and the airtime saving by itself provably
cannot move a ratio bar; COP-2 measured both facts (see `BENCHMARKS.md`'s
COP-2 section and `docs/adr/008-phase3-system-layer.md`'s COP-2 update).
Review finding: this is about the airtime component specifically, not short
CP overall — on a channel where the shorter CP changes decode success (FER),
that effect is NOT constant-factor and can move the ratio through the
resulting RateLoop trajectory. "Provably invisible to any ratio" overstates
what was measured; only the shared airtime factor is provably ratio-invisible.
