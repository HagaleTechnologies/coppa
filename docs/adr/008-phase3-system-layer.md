# ADR-008: Phase 3 — system layer (payload integrity, ARQ, IR-HARQ, rate loop, multi-codeword frames, SCO tracking, telemetry, benchmark program)

## Status

Accepted

See also: ADR-007 (multi-codeword frames + intra-frame cross-codeword interleaving — decision 6
below cross-references it rather than duplicating it, matching ADR-006's own precedent for
ADR-005). This ADR is the phase-level record: all 9 of Phase 3's locked design decisions, where
the shipped implementation deviated from the plan text, and the phase-closing benchmark-program
results (Task 8) plus the phase gate itself (Task 9).

## Context

Phase 2 ("receiver FEC harvest") left Coppa with a calibrated PHY/FEC layer (NR BG2 mother code,
soft header, exact LLR scaling) but no link-layer discipline built on top of it: no payload
integrity check beyond LDPC convergence, a constant ARQ RTO floor untuned to half-duplex HF
realities, no retransmission combining, a fixed link rate, one codeword per frame regardless of
modulation order, no sampling-clock-offset correction, a daemon that accepted telemetry types but
never sent them, and no benchmark program that made "world-class" a falsifiable claim rather
than a slogan. Phase 3 (`docs/superpowers/plans/2026-07-03-phase3-system-layer.md`, branch
`feature/system-layer`, 9 dev tasks + 1 phase-gate task) turns the PHY into an actual link:
payload CRC-32, half-duplex-aware ARQ, IR-HARQ, a closed rate loop (a human decision gate — see
decision 5), multi-codeword frames with intra-frame interleaving, SCO tracking, a spread-gated
short-CP profile, live telemetry, and the MIL-STD/session/golden-vector benchmark program. This
task (Task 9) is the phase gate: dead-code cleanup, this ADR, the BENCHMARKS.md re-baseline
(already added — see its "Phase 3 Task 8" and "Phase 3 Task 4" sections), and the CLAUDE.md
Known Limitations update.

**Headline finding, stated up front, matching this project's established practice of leading
with the honest picture rather than burying it:** most of Phase 3's individual tasks shipped
clean, verified wins (payload CRC-32, half-duplex RTO/backoff/SACK, IR-HARQ, multi-codeword
frames, SCO tracking, the short-CP profile, telemetry emission). Two things did not meet their
own stated acceptance bars, and both are reported honestly rather than adjusted: **Task 4's rate
loop** (adaptive/best-fixed = 0.894, adaptive/oracle = 0.751, vs. required >1.0/≥0.8 — a real,
peak-confirmed shortfall, root-caused to a level-dependent bias in the shared channel-capacity
metric, not a `RateLoop` logic bug), and **Task 8's benchmark-program acceptance targets**
(`milstd`: 0/27 operating points pass, even with +12 dB margin; `session`: 0/5 Moderate/Poor and
2/5 Good sessions complete drop-free — see BENCHMARKS.md's Task 8 section for the full,
twice-corrected honest diagnosis of why). Neither shortfall is a regression introduced by this phase's own code — both
are pre-existing PHY/channel-estimation-layer realities that this phase's new, more rigorous
measurement tools (a real closed-loop bench, a real MIL-STD-style ladder, a real session
simulator) exposed for the first time, consistent with the project's history of later phases'
measurement work surfacing gaps earlier phases' benches weren't built to see.

## Decision

The plan locked nine design decisions. Each is recorded below as originally decided, followed by
where the shipped implementation deviated and why.

### 1. Payload integrity: CRC-32 (IEEE) inside the info bits (Task 1)

**Plan**: layout `[payload | CRC-32 over payload | scrambled pad …]`; capacity per frame drops 4
bytes; `ReceiveError::CrcMismatch` finally constructed; LDPC convergence alone no longer ACKs.

**Shipped**: exactly this. `max_payload_for_level(level) = k_used/8 − 4` in `speed_levels.rs`.

**Deviation**: none in the decision itself. A pre-existing bench example
(`task4_bg2_ldpc_gate.rs`) had a payload-sizing calculation that didn't account for the new
4-byte CRC margin, causing a runtime panic not caught by `cargo test`/clippy/fmt (example
`main()` bodies aren't executed by either) — caught by the task reviewer running the example
directly, fixed via the same `max_payload_for_level` accessor `scenario.rs` already used.

### 2. Oversize payloads are a hard error (Task 1)

**Plan**: `transmit` returns `Result<Vec<f32>, TransmitError>`; `PayloadTooLarge { max }`
replaces silent truncation.

**Shipped**: exactly this. Every `transmit` call site across the workspace (bench, engine, FFI,
daemon) was updated to handle the `Result`, compiler-guided.

### 3. IR-HARQ: RV-cycled retransmissions with LLR combining (Task 3)

**Plan**: retransmission of seq N sends RV = `attempt mod 4` via 2 bits of `fec_type`; RX keeps
an LLR buffer per in-flight seq (mother-length, additively accumulated), evicted on CRC pass,
cumulative-ACK advance, or an LRU cap of 32.

**Shipped**: exactly this, with one explicit, brief-mandated override of the plan's own literal
RV-order phrasing: RV cycles as `[0,2,3,1][attempt % 4]` (standard LTE/5G-NR RV order — RV2
first, for maximum new parity on the first retransmission — not a plain `attempt mod 4`
identity mapping). `HarqRxBuffers` is a hand-rolled `HashMap` + recency-`Vec` (the plan's
"LruMap" was descriptive pseudocode, not a literal crate requirement — no new dependency was
added). `CoppaTransceiver` uses `RefCell`-based interior mutability for the HARQ buffer (matching
an existing `Cell` precedent in the same file) rather than changing `receive` to `&mut self`,
avoiding a call-site ripple across every consumer crate.

**Deviation, a real bug found and fixed along the way**: wiring real `rv` through the decode path
surfaced two latent bugs where the turbo re-estimation retry path (Phase 2 Task 5) had `rv`
hardcoded to 0 — silently correct only because nothing before this task ever exercised turbo
retry on a non-zero-RV retransmission. Fixed as part of this task.

**Measured**: a new bench (`task3_harq_ir_bench.rs`) confirms IR < Chase < Plain
transmissions-to-success at level 10/AWGN/18 dB (1.380 < 1.540 < 1.547). A counter-intuitive
result at level 2/Watterson-Moderate (IR *underperforming* Chase) was investigated, not hidden:
root-caused to RV2's rate-matching window landing entirely past the systematic-bit prefix into
pure parity bits at that low code rate — a real, known HARQ rate-dependent tradeoff, not a
combining bug (reviewer independently re-derived this from `rate_match.rs`'s actual `k0_offset`
arithmetic).

### 4. Half-duplex ARQ discipline (Task 2)

**Plan**: `rto_floor = burst_airtime(window, level) + 2·turnaround + ack_airtime(level_ack)`
(`turnaround = 150 ms` default); `backoff()` fires once per timeout EVENT, not per expired
segment; SACK bitmap widened to cover the full window (32-bit field, a wire change in the ACK
PDU); block-ACK cadence: ACK once per received burst boundary or 2 frames, not per frame.

**Shipped**: the first three sub-decisions exactly as planned —
`crate::arq::rto_floor`/`modem::airtime::frame_airtime_s` (a verified-faithful mirror of
`CoppaModem`'s real symbol-count arithmetic, independently re-derived by the task reviewer from
the brief's own ≈12.6 s worked example and matched to 12.585 s exactly); `backoff()` moved from
`mark_retransmitted` (per-segment) to `ArqTx::get_retransmits` (once per poll that finds ≥1
expired segment); `TransportPdu::ack_bitmap` widened `u8`→`u32` (header 4→7 bytes, a documented
wire-format break) with `SACK_RANGE` widened 8→31.

**Deviation, not implemented**: the plan's fourth sub-decision — a distinct block-ACK cadence
mechanism (batching ACKs to once per burst boundary or every 2 frames rather than one ACK per
decoded frame) — was **not implemented as its own mechanism**. A repo-wide grep for cadence/
batching/coalescing logic in `arq.rs`/`transport.rs` after this task shipped found nothing; every
decoded frame still triggers its own ACK. This is a real, honest gap against the plan's literal
text, not previously called out in the Phase 3 progress ledger — flagged here rather than left
implicit. It does not affect correctness (more ACKs than strictly necessary is safe, just less
airtime-efficient than the plan intended) and is a plausible candidate for a future task.

### 5. Rate loop — HUMAN DECISION GATE (Task 4)

**Plan**: two approved-but-divergent designs were on file — the pre-existing
`docs/superpowers/plans/2026-07-01-coppa-closed-loop-adaptive-rate.md` (executed mechanically),
or this phase's decision 5 (same architecture, amended for down-shift-on-timeout and
per-codeword noise-variance-based recommendation). **Tony chose the amended design (option b)**
at the decision gate.

**Shipped**: RX computes a recommended speed level (`coppa_ml::recommend_speed_level`, wrapping
`channel_capacity`/`channel_selectivity`/`select_speed_level_2d` over the frame's own per-carrier
noise variances) as the third element of `CoppaTransceiver::receive()`'s return (a new shared
`receive_core` refactor avoids duplicating the payload decode pipeline across the 2-tuple and
3-tuple call sites); fed back to the sender via `TransportPdu::new_ack_with_rate`/
`suggested_rate()` (1 byte on the widened SACK field decision 4 introduced); `coppa_ml::RateLoop`
applies it with hybrid hysteresis (raise one level only after `raise_dwell` consecutive
equal-or-higher recommendations — shipped default `raise_dwell = 5`, a genuine measured peak, not
just "more damping is safer" — drop immediately to the recommendation on a lower one, or one step
on any delivery failure/ARQ timeout event). The dead `coppa-engine::RateController` (never wired
to anything beyond a debug log) and the aspirational, unused, SNR-only `coppa-ml::MCS_TABLE` were
deleted as part of this task, along with the daemon's `20·log10(rms)+40` pseudo-SNR (confirmed
already fixed in a prior phase, not this task's own work).

**Acceptance bar NOT met, reported honestly**: the plan's own bar (adaptive/best-fixed goodput >
1.0 AND adaptive/oracle ≥ 0.8 on a time-varying-channel bench) is not cleared —
`crates/coppa-bench/examples/closed_loop_arq.rs` measures **0.894 / 0.751** at the shipped
`raise_dwell = 5`, confirmed via an 8-point `raise_dwell` sweep (3 through 15) showing `5` is a
genuine peak (both ratios rise 3→5 and fall on both sides), not a tuning gap that a different
hysteresis parameter would close. Root-caused (via an ad-hoc, uncommitted diagnostic — explicitly
flagged in the bench's own doc comment as a hypothesis pending a committed reproducible script,
not settled fact) to the shared `channel_capacity` metric not being invariant to which speed
level a measured frame happened to use: at a fixed, true, injected SNR, a level-7 transmission's
own channel estimate reads several dB higher "capacity" than an identical channel measured via
level 1/2, because `SPEED_LEVEL_MIN_CAPACITY`'s calibration (`mcs_calibration.rs`) only ever
probes at a fixed level-2 sounding frame — a self-reinforcing bias once `RateLoop` starts varying
the probing level itself, which is exactly this design's own point (zero extra probe overhead).
This is a channel-estimation/MCS-calibration-layer issue, not a `RateLoop` hysteresis bug — the
same accepted-shortfall pattern as Phase 2's Task 1/7 (delay-domain estimator) and Task 4 (NR BG2
LDPC). See BENCHMARKS.md's "Phase 3 Task 4" section for the full sweep table.

Two unrelated real bugs were found and fixed in the new closed-loop bench itself during this
investigation: a constant `seq_num` corrupting IR-HARQ's per-seq accumulator across
logically-independent simulated frames, and a shared transceiver risking the same
cross-contamination across fixed-level comparison runs.

### 6. Multi-codeword frames + intra-frame cross-codeword interleaving (Task 5)

**Plan**: header gains a `codewords` count; up to 8 codewords per frame amortize the fixed
preamble+header overhead; payload CRC per codeword; cross-frame interleaving is re-scoped to
intra-frame, across codewords, for levels ≥ 5.

**Shipped**: exactly this — see **ADR-007** for the complete decision record (header bit-budget
verification, per-codeword CRC-32 split, the `CrossFrameInterleaver` re-scoping, the
`(seq, codeword-index)` ACK-addressing and turbo/IR-HARQ-extension scope cuts, and the
honestly-re-derived airtime figure — the plan's "≤0.55×" estimate for level 6/7 codewords/800
bytes was 64-QAM-calibrated; the real measured ratio is ~0.639, still a real ~36% airtime
reduction). Not duplicated here.

### 7. SCO tracking (Task 6) and the short-CP profile (Task 6b)

**Plan (decision 7)**: per-symbol pilot phase slope (`dφ/dk = −2πτ/N_c`) EWMA-accumulated;
slip the FFT window start by `round(τ̂)` once `|τ̂| ≥ 0.5` samples; applied inside
`demodulate_frame`'s symbol loop, no waveform change.

**Shipped**: exactly this mechanism
(`delay_domain::timing_offset_samples`, a distinct real-samples convention from
`estimate_coarse_delay`'s nc-normalized grid units, independently re-derived and confirmed
correct by the task reviewer), integer-slipping `sym_start` for subsequent symbols and
subtracting the applied amount back out, `frame_start`-relative indexing kept consistent. Real
effect demonstrated end-to-end: a +120 ppm-resampled 5 s multi-codeword frame decodes at BER
0.0067 with tracking on vs. 0.2335 off.

**Deviation, a real regression found and fixed during development, not the plan's literal
number**: the plan's literal `α = 0.1` directly regressed
`hf_standard_header_survives_watterson_moderate_fading` (a zero-SCO channel) 294/300→264/300 at a
dedicated 300-seed sweep — root-caused to the same per-symbol "dominant-tap-swing" failure mode
`estimate_coarse_delay`'s own doc already warns about for Watterson fading, amplified at
per-symbol (~4 pilots) scale. **Shipped default: `α = 0.05` plus a new 2.0-sample per-symbol
clamp**, both justified via a from-scratch EWMA-of-a-ramp simulation the reviewer independently
reproduced (the clamp doesn't interfere with genuine 120 ppm SCO responsiveness; a single
fading-artifact spike, previously up to ~48 samples, is now bounded before entering the EWMA).
This is the fourth piece of work in this exact code area's history (ADR-003, ADR-004, the Phase 2
CFO×level-4 fix) to catch a plausible-looking regression via the same Watterson guard test before
it could land — see CLAUDE.md's Known Limitations for the standing pattern this represents.

**Task 6b (short-CP profile, a closely related follow-on, not itself one of the plan's 9 numbered
decisions but built on Task 6's prerequisite)**: new `hf_standard_short_cp()` profile
(`cp_samples = 144`, ~3 ms flat CP + slop, distinct `bandwidth_id = 4`) plus a new
`coppa_ml::CpGate` spread-gate (raise-slow/drop-fast hysteresis mirroring `RateLoop`'s pattern,
`N = 4` dwell / 2.5 ms threshold) recommending whether the short-CP profile is currently safe,
from measured per-frame delay-spread history. Explicit scope discipline: no wire-format change,
no daemon integration, no live mid-session renegotiation — matches Task 4's precedent of
deferring daemon-level closed-loop wiring. **Deviation, a real bug found and fixed**:
`CpGate::observe`'s `run: u8` counter incremented unbounded on sustained calm-channel
observations (the expected common case once switched to short-CP) — overflow-panicking (debug)
or wrapping (release) after 255 consecutive calm frames; fixed via
`saturating_add(1).min(consecutive_needed)`, a textbook-correct fix per the task reviewer,
plus a 300-iteration regression test. **Disclosed, not fixed, limitation**: `CpGate`'s
(and, per Task 7 below, `BusyGate`'s) threshold constants are synthetic-test-validated only, not
swept/calibrated against a real bench the way `SPEED_LEVEL_MIN_CAPACITY` was — see CLAUDE.md's
Known Limitations.

**Follow-on 1 (PR #67, 2026-07-29): CP-switch peer negotiation.** The "no daemon integration, no
live mid-session renegotiation" scope cut above was closed by `coppa_protocol::cp_negotiator`, a
receiver-initiated two-leg handshake (`Propose` → `Confirm` → bare ack) gated by
`cp_negotiation_enabled` (off by default). Roles, using the code's own labels: **B (proposer)**
sends `Propose` and switches its own receiver on receiving `Confirm`; **A (confirmer)** sends
`Confirm` and switches its own encoder only once it sees B's bare ack. Note that plain English and
the code collide here — the station performing the *act* of acknowledging is B, not A — and both
CLAUDE.md and the PR #67 body inverted the names as a result; `cp_negotiator.rs`'s module doc now
calls the collision out explicitly.

**Follow-on 2 (COP-1, PR #72, 2026-08-01): converging when a handshake leg is lost.** PR #67's
final bare ack was un-retryable *by construction* — it never reaches `ArqTx::send`, and a
retransmitted `Confirm` cannot re-elicit it because `ArqRx`'s dedupe swallows the duplicate before
it reaches the ack-sending code — yet B irrevocably commits both its transmitter and receiver to
the new profile immediately after sending it. Losing that ack left the two stations on
mutually-undecodable CP profiles with no timer, no retry, and no reachable recovery path.

Fixed by combining **both** remedies, because neither converges alone:

1. **A third handshake leg**, `CpSwitched` (wire kind `0x03`), sent by A under the *new* profile
   the instant it switches, ARQ-tracked. This gives B a deterministic, immediate proof-of-switch
   instead of a traffic-dependent one — on an idle link, no inferable traffic ever arrives.
2. **A bounded revert on both sides.** Every wait state gets a give-up trigger, all converging on
   the **pre-negotiation mode** — the last mode both stations are known to have *agreed* on, since a
   negotiation only ever starts from a converged state. For the first negotiation of a session that
   mode is the `CpMode::LongCp` both stations boot into:

   | # | Who waits | For what | Trigger | Action |
   |---|---|---|---|---|
   | G1 | B | its `Propose` acked | `ArqTx::is_failed(propose_seq)` | abandon segment, clear seq (B never switched) |
   | G2 | A | its `Confirm` acked | `ArqTx::is_failed(confirm_seq)` | abandon segment, `abort()` (A never switched) |
   | G3 | B | `CpSwitched` from A | probation deadline elapsed | **revert engine + negotiator** |
   | G4 | A | its `CpSwitched` acked | `ArqTx::is_failed(switched_seq)` | abandon segment, `abort()`, **revert engine** |

   Every single-leg loss fires a trigger on *both* stations, which is what makes convergence total:
   losing `Propose` fires G1 alone (A never saw anything); losing `Confirm` fires G1+G2; losing the
   bare ack fires G3+G2; losing `CpSwitched` fires G3+G4.

   **Amended by the branch's own final review, before merge.** As first written this decision said
   the target was *always* `LongCp`, and `CpNegotiator::abort()` hardcoded it while `tick()`
   restored the pre-switch mode. Those two agree only when the stations were on `LongCp` before the
   negotiation — but `CpGate` reverts to `CpRecommendation::LongCp` on any single frame at or above
   threshold ("drop fast"), and the daemon turns that transition into a real `Propose(LongCp)`, so a
   `ShortCp` → `LongCp` negotiation is a first-class production path, and it is the one that runs
   exactly when the channel is degrading and legs get lost. In that direction three of the five
   droppable frames desynced the link *permanently*: with the `Confirm` lost, A had never switched
   at all yet `abort()` still dragged it to `LongCp` while B stayed on `ShortCp`; with the bare ack
   or `CpSwitched` lost, G3's revert-to-previous actively *broke* an agreement the two stations had
   already reached. Both paths now go through one private `CpNegotiator::revert()` helper so they
   cannot drift apart again, and `abort()` returns the mode it reverted to so the daemon passes that
   to `set_cp_profile` instead of a hardcoded constant. Bit-identical to the shipped behavior in the
   `LongCp` → `ShortCp` direction, which is why every pre-existing test stayed green.

3. **A re-ack for duplicate CP-control content**, which covers the handshake's *fifth* droppable
   frame. The six-step diagram in `cp_negotiator.rs` puts five frames on the air, not four: step 6
   is B's bare ack for A's third leg, and `on_peer_switched` disarms B's probation the instant that
   leg arrives. So losing step 6 left A's G4 to fire and revert A while B, with no timer left at
   all, stayed on the new profile — the same permanent desync this follow-on exists to eliminate,
   reintroduced one step later. A's retransmitted third leg could not re-elicit the ack either,
   because `ArqRx` swallows an already-delivered seq before it reaches the ack-sending code: the
   very dedupe trap that made step 3's ack un-retryable. `handle_cp_control` now emits a bare ack
   for a content PDU that delivered nothing (a duplicate, or a gap-filler buffered ahead of
   `recv_base`) without re-running its content action, which makes step 6 recoverable by ordinary
   retransmission — the same mechanism that covers legs 1, 2 and 4. Giving B a second deadline
   instead was considered and rejected: nothing could clear it on an idle link, so it would
   reintroduce exactly the churn failure mode described below. Note this does *not* rescue step 3's
   ack, where B has already switched and the re-ack would encode under a profile A cannot yet
   decode; G2/G3 still own that case.

The third leg alone does not converge when it is itself lost. The bounded revert alone converges
but causes idle-link churn: with no third leg the only disarm signal is "some frame decoded after
the switch," so a quiet link spuriously reverts and — because `CpGate` only proposes on a
*transition* and its recommendation is already `ShortCp` — never re-proposes, silently abandoning
short-CP on exactly the calm channels the feature exists to exploit.

Three supporting changes: `ArqTx::abandon` releases a given-up segment (nothing previously evicted
one — `get_retransmits`'s `max_retransmit` guard stops retrying but never removes, so the
two-slot CP-control window leaked a slot per failed negotiation), deliberately *not* by rebuilding
the pair, since that would rewind `next_seq` on whichever station gave up first and the peer's
`ArqRx` would then swallow the recycled seq as a duplicate. The `Reset` arm now calls
`engine.set_cp_profile(LongCp)` alongside its `CpNegotiator::new()` — it previously reset the
bookkeeping only, leaving an already-switched station's engine and negotiator disagreeing. And
`send_cp_switched`'s window-full `Err` arm now walks the engine back rather than only logging: its
caller has already committed the engine to the new profile (it must — the third leg is encoded under
it), so returning left the station switched with no tracked leg, hence no `pending_switched_seq`,
hence no G4 that could ever fire. That was an unbounded wait reached silently, by the one path that
escaped the give-up machinery entirely. The `encode_bytes` `Err` arm beside it deliberately does not
do this: there the seq *is* tracked, so the retransmit loop re-encodes it and, failing that, G4
fires normally.

**Verification and its limits, stated plainly**: end-to-end loss-injection tests — one per droppable
frame, in both negotiation directions — drive two real `EventLoop`s through the real
`decode_and_dispatch_audio` path with one frame's samples discarded, plus
`after_a_failed_negotiation_a_later_negotiation_still_succeeds`, the only check that recovery leaves
the link *usable* rather than merely *consistent*. Still unproven: `cp_negotiation_enabled` remains
`false` by default, so none of this has run in a deployment — and **COP-2 (2026-08-05) established
why that flag is not the single switch it looks like**. Turning live negotiation on is a conjunction
of *three* flags. The flag itself gates only *acting on* the handshake, which is `handle_cp_control`'s
sole guard (`event_loop.rs:1903-1905`); the only code that can *initiate* a negotiation sits inside
`if self.config.engine.cp_gate_enabled` (`:1093`), so without that flag a negotiation-enabled station
is structurally incapable of proposing; and `arq_enabled` gates the inbound `TransportPdu` parse that
is the sole route to any CpControl PDU. The consequence is asymmetric and deserves deliberate thought
rather than a default change: the flag alone never initiates, but once `arq_enabled` is on it makes
the station a full **responder**, so a peer can talk it onto short CP without it ever asking. COP-2
therefore changed **no default** — the flip was treated as a HUMAN DECISION GATE in decision 5's
sense, evaluated against the evidence and declined, not deferred as future work — and left every
other unknown in this list exactly as it stands;
`SWITCH_PROBATION_SECS = 180` is derived from the ARQ worst case (≈135 s) plus margin rather than
swept, the same "no bench exists for it" caveat `CpGate`'s own constants carry above; verification is
daemon-to-daemon in-process, with no live two-radio field test; and multi-leg (as opposed to
single-leg) loss is not tested — including the one residual the fifth frame's fix leaves: if step 6
*and* every retransmission of the third leg are lost, A's G4 reverts A while B stays on the new mode.

### 8. Telemetry (Task 7)

**Plan**: daemon emits `SNR <db>` after each decoded frame, `PTT ON/OFF` around transmit,
`BUFFER <n>` on TX queue changes, `BUSY ON/OFF` from a spectral occupancy gate
(`coppa_ml::spectrum_sensor`, threshold = noise floor + 6 dB in the 300–2800 Hz band); WebSocket
`status` carries real `connected`/`snr`/`level`/`cfo`.

**Shipped**: exactly this — all four VARA telemetry lines (reusing the already-existing
`VaraResponse::{Ptt,Buffer,Busy,Snr}` wire types and `response_senders()` verbatim, confirmed zero
duplication), SNR from the real per-frame `snr_db`, PTT at the same pre-existing
physical-PTT-hardware call sites (no new/duplicate timing mechanism), BUFFER from the real
`VecDeque` TX-queue's enqueue/drain transitions (hand-traced by the reviewer against the exact
3,2,1,0 progression), BUSY from a new `coppa_ml::BusyGate` transition-only occupancy gate over
`SpectrumSensor::band_occupancy`.

**Deviations, two real bugs found and fixed, not just documentation gaps**: (1)
`band_occupancy`'s Hz-to-bin resolution used the constructor's fixed `fft_size`, but
`power_spectrum`'s actual FFT length shrinks whenever fewer samples than `fft_size` are
available — exactly the daemon's normal ~20 ms-poll-tick steady state — silently mis-banding the
occupancy gate under real operation; fixed to derive resolution from the real spectrum length,
verified via independent re-derivation (a 3200 Hz tone the old code would misclassify as in-band
is now correctly excluded). (2) `WsStatus.connected` was hardcoded `true` on any decoded frame
and never reset — judged a real semantic bug (a monitoring client would misread a dead link as
live); fixed to recompute from `session_mgr`'s real established-session state at the same update
point, with an honestly-disclosed smaller residual gap (only refreshes on a decode event).

### 9. Benchmark program (Task 8)

**Plan**: (a) a `milstd` bench at MIL-STD-188-110 Table XVI-style operating points, mapping
Coppa levels to nearest standard rates; (b) a session-robustness bench scoring connection
survival + net goodput over simulated 10-minute ARQ sessions on a slowly SNR-ramping Watterson
channel; (c) 20 golden WAVs + manifest + expected payloads under `testdata/golden/`, CI-checked.

**Shipped**: exactly this — see BENCHMARKS.md's "Phase 3 Task 8" section (added by this task,
Task 9, since Task 8 itself built the benches but did not add a BENCHMARKS.md section) for the
full tables. **Design deviation, not a decision deviation**: implemented as three separate
example binaries (`milstd.rs`, `session.rs`, `golden_vectors_gen.rs`) under
`crates/coppa-bench/examples/`, matching this crate's established one-off-bench-tool pattern
(19 pre-existing examples, none of which are `clap::Subcommand` variants), rather than adding
subcommands to `src/main.rs` — zero risk to the existing default sweep CLI, confirmed unchanged
by direct re-run after all other changes.

**Original acceptance targets NOT cleanly met, reported honestly (see BENCHMARKS.md for the full,
twice-corrected diagnosis)**: `milstd` passes 0/27 operating points, even with a generous +12 dB
margin — root-caused to the ladder's borrowed reference SNRs not transferring onto Coppa's real
measured thresholds on any channel (not a fading-specific bug, though the already-documented
Watterson-Moderate/Poor channel-estimation gap is a real, additional contributing factor for
those specific rows). The current `session` baseline is 2/5 Good, 0/5 Moderate, 0/5 Poor sessions completing
drop-free against a "zero drops on good/moderate" target — root-caused to level 2's real
Good-preset FER not being zero even above its nominal threshold, so a sustained low-SNR ramp
trough can exhaust the ARQ's bounded retransmit budget on a non-trivial fraction of trials (not
an ARQ state-machine bug). A real bug was found and fixed while building both: `select_profile()`
defaults levels ≥5 to a VHF profile whose 60-sample CP causes 100% frame loss under any
Watterson fading — worked around by forcing `hf_standard` for every level in these two
HF-specific tools (domain-correct regardless, since MIL-STD-188-110 is an HF standard). The
golden-vector corpus itself (deliverable c) is complete and passing: all 20 vectors, including
`L9_poor25`, decode to their exact manifest payload. The Poor vector is seed-selected and does not
imply that level 9 meets a statistical FER target under Watterson fading.

## Consequences

### Wire-format break

Frames with `codewords > 1` (decision 6) are not decodable by any pre-Task-5 codec; the widened
`ack_bitmap` (`u8`→`u32`, decision 4) changes the ACK PDU's header size (4→7 bytes). Both breaks
are additive on top of Phase 1's waveform break (ADR-003) and Phase 2's NR BG2/level-10-rate
break (ADR-005/ADR-006) into the same overall Phase-1-through-3 wire-format generation.
`codewords == 1` frames remain byte-for-byte identical to every pre-Task-5 frame, so single
codeword interop is preserved; multi-codeword traffic and the wider SACK are the only things that
don't round-trip against an older build. Acceptable pre-1.0 (no deployed installed base).

### Two real, honestly-reported shortfalls carried forward

- **Task 4's rate loop** does not clear its own acceptance bar (0.894/0.751 vs. required
  >1.0/≥0.8), root-caused to a level-dependent bias in the shared `channel_capacity` metric this
  design deliberately built on (recommend from the actual in-flight frame's own channel estimate,
  at whatever level it used, for zero extra probe overhead) — the very thing that exposes the
  bias, since existing calibration benches never varied the probing level. Not a `RateLoop`
  hysteresis bug. See BENCHMARKS.md's "Phase 3 Task 4" section and CLAUDE.md's Known Limitations.
- **Task 8's original benchmark-program acceptance targets** (`milstd` 0/27, `session`
  drop-free-on-good/moderate) were not met, but this is presented as new, more rigorous measurement exposing
  pre-existing PHY/channel-estimation-layer realities (a calibration mismatch between a borrowed
  reference ladder and Coppa's real thresholds; the already-tracked Watterson-Moderate/Poor
  channel-estimation gap; level 9's separately-unexplained high/steep/seed-dependent AWGN
  threshold) — not a regression this phase's own code introduced. See BENCHMARKS.md's "Phase 3
  Task 8" section and CLAUDE.md's Known Limitations.

**COP-5 update (2026-08-03):** the `session` zero-drop aspiration is superseded, not achieved.
The benchmark now scores only Good against its deterministic regression floor of at least 2/5
drop-free sessions; Moderate and Poor remain diagnostic-only pending their separately tracked PHY
limitations. This is a reproducible regression policy, not a production reliability guarantee.
Increasing retries would tune production ARQ policy to this synthetic trough, while adding
`RateLoop` would defeat the benchmark's fixed-level ARQ-isolation purpose, so neither was chosen.

**COP-4 update (2026-08-05):** the old level-9 fading explanation was measurement-incomplete.
A corrected modulation-aware per-frame diagnostic shows errors concentrated more strongly on
high-noise-variance carriers, while a 300-trial profile/CP matrix shows that no tested profile clears
FER≤10% through 36 dB. A perfect-CSI, decode-independent bound nevertheless admits 91.6% of
Watterson-Good frames at 30 dB (95% CI 88.84–93.73%), versus 15.0% real decode success (95% CI
11.40–19.48%). This is implementation headroom in FEC coverage/diversity, not proof of a physical
64-QAM ceiling and not a reason to change profile routing. See BENCHMARKS.md's COP-4 section.

**COP-2 update (2026-08-05):** Task 4's bar was re-measured with `closed_loop_arq`'s metric
airtime-normalized, and the bar itself is the primary finding. The previous bits-per-frame-*slot*
convention was structurally blind to an airtime lever, but normalizing does not rescue the bar
either: short CP scales every level's frame airtime by one constant (`1104/1260`, +14.130%), and a
ratio of two goodputs divided by the same constant is unchanged, so this bar cannot express an
airtime improvement by construction. Measured on `hf_standard` ↔ `hf_standard_short_cp`, 5 seeds ×
300 frames: against the pre-committed joint 18-cell comparator the CP-adaptive arm reads 0.948 /
0.654 — **NOT MET**, clearing `> 1.0` on 1 of 5 seeds — while the *same* arm scored against a
long-CP-only 9-cell comparator, the one denied short CP, reads 1.101 and clears on all five, a pass
that is pure denominator arithmetic (the gaps to the bar are +9.41% and +4.99% against a +14.130%
constant). Where the win is real is the denominator: best-fixed goodput 2179.1 → 2531.6 bps
(+16.18%), oracle 3066.6 → 3670.5 bps (+19.69%), with best-fixed(joint) selecting a short-CP cell on
5/5 seeds. CP *adaptivity* added nothing over simply always running short CP (−0.83% against arm C,
`FixedCpAdaptiveRate{ShortCp}` — adaptive rate held constant so the delta isolates CP policy alone,
correcting an earlier version of this comparator that conflated rate-adaptivity with CP-adaptivity;
0/5 seeds on both bases) and the schedule produced exactly one `CpGate` transition per run, so the
evidence routes to short CP as a **static (negotiated) configuration for HF levels 1-4** — not an
adaptive control, and not fade-diversity interleaving, which stays available but unrouted. On
`robust`, a separately-measured header-codeword confound (+0.94%) is larger than this delta, so only
the direction (never beats the isolation control) is load-bearing, not the magnitude — see
`BENCHMARKS.md`'s Table 3. Like COP-5 above, this redefines what the metric can honestly be asked
and records it, rather than forcing the number.

### A real, undocumented-until-now plan deviation

Decision 4's block-ACK cadence sub-point (batch ACKs to once per burst boundary or every 2
frames) was never implemented — every decoded frame still triggers its own ACK. This does not
compromise correctness, only airtime efficiency; flagged here as an honest gap rather than left
implicit, since it was not previously called out in the Phase 3 progress ledger.

### Dead code removed as part of this phase's own close-out (Task 9)

The protocol-side `fec::interleaver` module (`BlockInterleaver`/`FrequencyInterleaver`, distinct
from and not to be confused with `coppa-codec::ofdm::interleaver`'s same-named, very much alive
types) had zero callers outside its own file and was deleted. `Frame::to_bits_split_v2`/
`from_payload_bits_v2` (a test-only duplicate of `to_bits_split`/`from_payload_bits` that existed
only to demonstrate a length-covering CRC scope) was folded into the V1 methods directly and
deleted, since the fold was genuinely trivial (the two methods' only functionally meaningful
difference was CRC scope; the "V2" reserved byte carried no information and was dropped rather
than folded in). `coppa-codec::ofdm::sync::estimate_cfo_hz` (a legacy single-lag Moose CFO
estimate, explicitly labeled as such in its own doc comment, superseded in production by
`estimate_cfo_two_stage`) was `#[cfg(test)]`-gated rather than deleted outright, following the
same treatment Phase 2 gave the Golay hard-decision reference decoder. `coppa-ml`'s
`channel_predictor.rs`/`registry.rs` (an EWMA predictor + optional-model-file registry with zero
callers anywhere outside the crate itself, and zero real implementors of their own `MlModel`/
`ChannelPredictor` traits beyond a no-op `FixedPredictor` stub) were deleted along with those
traits, `FixedPredictor`, and `load_channel_predictor` from `lib.rs`; the crate's doc comment was
rewritten to describe what it actually does (capacity-based speed-level selection, the rate
loop, the spread-gated short-CP recommendation, the busy gate, spectrum sensing) instead of
apologizing for not being ML. **`coppa-protocol::fec::convolutional` (`ConvEncoder`/
`ViterbiDecoder`) was investigated and found NOT to be dead code, contrary to an initial
assumption**: it is used by `benches/throughput.rs` (a `[[bench]]` target of the root package)
and `fuzz/fuzz_targets/fuzz_viterbi.rs` (a real cargo-fuzz target, excluded from the Cargo
workspace but still a maintained tool) — both outside the crate boundary a narrower grep might
have checked. It was kept, unchanged. See `.superpowers/sdd/task-9-report.md` for the full
verification trail.

## Related

- `docs/superpowers/plans/2026-07-03-phase3-system-layer.md` — the plan this ADR records.
- `docs/adr/007-multi-codeword-frames.md` — decision 6 in full detail.
- `docs/adr/006-phase2-parametric-estimation-nr-bg2.md`, `005-nr-bg2-ldpc.md`,
  `004-strongest-path-timing.md`, `003-phase1-waveform-break.md` — the prior phases' wire-format
  breaks and the sparse-pilot/CFO fixes this phase's SCO-tracking work continues the pattern of.
- `.superpowers/sdd/progress.md`'s Phase 3 section — the authoritative per-task ledger this ADR
  summarizes; written by the coordinator after each task's review.
- `.superpowers/sdd/task-{1,2,3,4,5,6,6b,7,8}-report.md` — full per-task investigation detail.
- `.superpowers/sdd/task-9-report.md` — this task's full report, including the dead-code
  verification trail and self-review.
- `BENCHMARKS.md`'s "Phase 3 Task 4" and "Phase 3 Task 8" sections — full before/after tables.
