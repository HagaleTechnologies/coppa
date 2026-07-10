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
2/5 Good sessions drop — see BENCHMARKS.md's Task 8 section for the full, twice-corrected honest
diagnosis of why). Neither shortfall is a regression introduced by this phase's own code — both
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

**Acceptance targets NOT cleanly met, reported honestly (see BENCHMARKS.md for the full,
twice-corrected diagnosis)**: `milstd` passes 0/27 operating points, even with a generous +12 dB
margin — root-caused to the ladder's borrowed reference SNRs not transferring onto Coppa's real
measured thresholds on any channel (not a fading-specific bug, though the already-documented
Watterson-Moderate/Poor channel-estimation gap is a real, additional contributing factor for
those specific rows). `session` shows 3/5 Good, 0/5 Moderate, 0/5 Poor sessions completing
drop-free against a "zero drops on good/moderate" target — root-caused to level 2's real
Good-preset FER not being zero even above its nominal threshold, so a sustained low-SNR ramp
trough can exhaust the ARQ's bounded retransmit budget on a non-trivial fraction of trials (not
an ARQ state-machine bug). A real bug was found and fixed while building both: `select_profile()`
defaults levels ≥5 to a VHF profile whose 60-sample CP causes 100% frame loss under any
Watterson fading — worked around by forcing `hf_standard` for every level in these two
HF-specific tools (domain-correct regardless, since MIL-STD-188-110 is an HF standard). The
golden-vector corpus itself (deliverable c) is complete and passing: 19/20 vectors decode to
their exact manifest payload; the 20th (`L9_poor25`) is committed with `expected_decode_ok =
false` as a deliberate, documented regression tripwire (level 9's Watterson-Poor non-convergence
is real and structural, verified to 54 dB — not something a different seed/payload could fix).

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
- **Task 8's benchmark-program acceptance targets** (`milstd` 0/27, `session` drop-free-on-
  good/moderate) are not met, but this is presented as new, more rigorous measurement exposing
  pre-existing PHY/channel-estimation-layer realities (a calibration mismatch between a borrowed
  reference ladder and Coppa's real thresholds; the already-tracked Watterson-Moderate/Poor
  channel-estimation gap; level 9's separately-unexplained high/steep/seed-dependent AWGN
  threshold) — not a regression this phase's own code introduced. See BENCHMARKS.md's "Phase 3
  Task 8" section and CLAUDE.md's Known Limitations.

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
