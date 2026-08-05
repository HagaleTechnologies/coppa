---
id: cp-negotiation-gotchas
title: What will bite you about the CP-switch negotiation handshake?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-protocol/src/cp_negotiator.rs
  - crates/coppa-protocol/src/arq.rs
  - crates/coppa-daemon/src/event_loop.rs
  - crates/coppa-daemon/src/config.rs
  - crates/coppa-protocol/src/modem/airtime.rs
  - docs/adr/008-phase3-system-layer.md
verified:
  commit: 2efc1c1
  date: 2026-08-05
links:
  - adr-008-phase3-system-layer
  - coppa-protocol
---
Five things about the CP-switch negotiation handshake have each already cost
real debugging time, and none of them is visible from a casual read of the
code. The handshake itself is documented in `cp_negotiator.rs`'s module doc and
in `docs/adr/008-phase3-system-layer.md` (section 7) — this page only records
the traps, which span `coppa_protocol::cp_negotiator` and the `coppa-daemon`
config flags that gate it.

## 1. The A/B role names are a genuine collision, and prose has inverted them twice

The code defines **B = proposer** (sends `Propose`) and **A = confirmer**
(sends `Confirm`). But the station performing the plain-English act of
*confirming* — acknowledging that it heard and accepted — is **B**, not A. B's
own action variant is named `ContentAction::ApplyAsConfirmer` and its tracing
string says `"CP profile switched (proposer role, own receiver)"`, which reads
as a contradiction until you notice the collision.

Both `CLAUDE.md`'s Known Limitations bullet and the PR #67 body inverted the
roles as a direct result, and both had to be corrected in COP-1.

**Rule: trust the wire direction, not the word.** A sends the payload literally
named `Confirm`; B receives it. The `ApplyAsConfirmer` variant name is left
alone deliberately — renaming it would touch every call site, both tracing
strings, and every test for zero behavior change — so the doc carries the
correction instead of the identifier.

## 2. A retransmitted `Confirm` cannot re-elicit a lost bare ack

This is the trap that makes the obvious cheap fix wrong. The handshake's final
bare ack is not ARQ-tracked, so the natural instinct is "the existing
retransmit loop will eventually re-trigger it." It cannot, and this is
structural, not a tuning problem:

- `ArqRx::receive` returns an empty `delivered` for an already-delivered seq
  (`arq.rs`), deliberately, so a duplicate CpControl PDU is a pure no-op.
- The bare ack is only ever emitted from inside the
  `for (_seq, data) in delivered` loop in `EventLoop::handle_cp_control`.

So the duplicate is swallowed by the dedupe *before* it can reach the
ack-sending code. The existing retransmit machinery is **inert** against this
gap, not a partial mitigation. COP-1 had to add a third handshake leg
(`CpSwitched`) precisely because nothing cheaper works.

**Amended by COP-1's own final review.** `handle_cp_control` now *does* emit a
bare ack for a content PDU that delivered nothing, so the dedupe no longer
blocks a re-ack outright. That does not rescue step 3's ack and does not make
the third leg redundant — by the time B would re-ack a retransmitted `Confirm`
it has already switched, so the re-ack encodes under a profile A cannot yet
decode. What it *does* rescue is the handshake's **fifth** droppable frame: B's
bare ack for the third leg, where both stations are already on the new profile
and the re-ack therefore lands. Without it, losing that frame left A's G4 to
revert A while B — probation already disarmed by `on_peer_switched` — stayed on
the new profile forever, i.e. the exact gap this page describes, one step later.
Read this section's "inert" claim as scoped to step 3 from here on.

Relatedly: B cannot simply defer its switch until it hears A on the new
profile, because `CoppaCore::set_cp_profile` rebuilds transmitter and receiver
*together* — there is no RX-only switch and no dual-profile receive. That same
coupling is what forced the `e59bf56` "send the ack first, then switch"
ordering; reversing it deadlocks the handshake in real use, which was found
only via real end-to-end audio testing, never by inspection.

## 3. `ArqTx::abandon` is not a pair rebuild, and must not be "simplified" into one

`abandon(seq)` exists because nothing else ever evicts a segment the sender
gave up on: `get_retransmits`'s `transmit_count <= max_retransmit` guard stops
*retrying* a dead segment but never *removes* it, and only `process_ack` clears
a slot. On the CP-control pair's `window_size: 2` that means one failed
negotiation leaves the pair half-dead and a second wedges it completely.

The tempting alternative — rebuilding the `ArqTx`/`ArqRx` pair the way the
`Reset` arm does — is wrong here, and subtly so. Rebuilding rewinds `next_seq`
to 0. The two stations give up at genuinely different moments (G2 is
ARQ-budget-driven, G3 is wall-clock-driven), so one can rebuild while the other
has not; their sequence spaces then diverge, and the next `Propose` at a
recycled seq is silently swallowed by the peer's `ArqRx` dedupe —
reintroducing a stuck handshake by a different route. `abandon` keeps both
sequence spaces monotonic regardless of give-up skew, which is the whole point.

## 4. Testing this needs the real audio dispatch path, and a same-value `set_cp_profile` workaround

Component tests that drive `CpNegotiator`/`ArqTx`/`TransportPdu` primitives by
hand will not catch bugs in the match-arm/retransmit wiring — the existing
`cp_negotiation_handshake_primitives_compose_correctly` says so in its own doc.
The loss-injection tests therefore drive two real `EventLoop`s through the real
`decode_and_dispatch_audio` path; loss injection is simply reading a leg's
samples out of the sender's ring and not delivering them.

Two things will bite you writing such a test:

- **Decoding a station's transmitted audio needs an engine built the same way
  the daemon builds its own.** `CoppaCore::new()` differs from the daemon's
  engine in `compression_enabled`, so decoding a daemon-transmitted frame with
  it silently yields the still-compressed bytes (leading marker byte `0xFE`)
  rather than the PDU. The pre-existing handshake test only asserts
  `decoded.is_ok()`, which is why this never surfaced there.
- **A station's *second* real decode of a `CoppaCore::encode_bytes`-built frame
  fails**, per the standalone `CoppaCore` known-limitation bullet in
  `CLAUDE.md` ("Bug B", still open). The workaround used throughout these tests
  is a same-value `engine.set_cp_profile(current_mode)` call, which rebuilds
  `self.streaming` and clears the stuck state without lying about which profile
  the incoming audio is encoded under. Carry it with its comment; do not chase
  that bug from CP-negotiation code.

## 5. Enabling CP negotiation is three flags, not one

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

## Footnote, not one of the traps: short CP cannot move a ratio

Anyone reaching for short CP to improve a goodput *ratio* should read
`coppa_protocol::modem::airtime`'s
`short_cp_scales_frame_airtime_by_one_constant_at_every_level` first. A uniform
CP change is one constant airtime divisor — the same `1104/1260` at every speed
level — so it cancels out of `adaptive/best-fixed`, `adaptive/oracle`, or any
other ratio of two goodputs measured under it. It is a real absolute-goodput
lever and a provably invisible one to a ratio bar; COP-2 measured both facts
(see `BENCHMARKS.md`'s COP-2 section and `docs/adr/008-phase3-system-layer.md`'s
COP-2 update).
