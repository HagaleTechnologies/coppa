# ADR-007: Multi-codeword frames + intra-frame cross-codeword interleaving

## Status

Accepted

## Context

Every frame before this task carried exactly one LDPC codeword. At high-order modulation the fixed
per-frame overhead (preamble + probe + protected header, ~7-19 OFDM symbols depending on level) is
large relative to one codeword's own payload span -- e.g. at level 6 (16-QAM) one codeword's payload
is only 12 OFDM symbols, so a single-codeword frame spends roughly 7/19 (~37%) of its airtime on
overhead that carries no payload bits at all. Phase 3's system-layer plan (decision 6,
`docs/superpowers/plans/2026-07-03-phase3-system-layer.md`) calls for batching up to 8 codewords
into one frame to amortize that fixed cost, and for reusing the already-built (but until now
bench-only) `CrossFrameInterleaver` to spread each codeword's coded bits across all codewords'
time-slots *within* that one frame, so a short, contiguous mid-frame fade damages every codeword a
little rather than one codeword a lot.

## Decision

1. **`CoppaHeader` gains a `codewords: u8` field, packed into byte 5's previously-`reserved:4`
   nibble as `codewords - 1`.** The header already had exactly 4 spare bits (verified before this
   task started by reading `frame.rs`'s doc/`to_bytes`/`from_bytes`) -- no header extension (the
   plan's contingency for "if not [enough spare bits], extend the header by one byte") was needed.
   Encoding `codewords - 1` rather than `codewords` directly means `codewords == 1` (every frame
   before this task, and the common case after it) encodes to nibble `0`, byte-for-byte identical
   to the pre-this-task `reserved: 0` wire format -- this is what makes the change backward
   compatible for single-codeword frames specifically, not a fully-general "any header change is
   fine" claim. The 4-bit field's full range is `codewords ∈ 1..=16`; only `1..=8` (the plan's
   budget) is produced/accepted by `CoppaTransceiver` today, the rest is unused wire headroom.

2. **Payload CRC-32 moves from "one CRC over the whole payload" to "one CRC-32 per codeword".**
   `CoppaTransceiver::transmit` splits the payload across `codewords` near-equal chunks
   (`split_payload_across_codewords`: `base = total/codewords`, the first `total % codewords`
   chunks get `base + 1` bytes, not the last chunk -- this bounds every chunk to at most
   `max_payload_for_level(level)` whenever the whole-frame oversize check passes, which dumping the
   remainder onto the last chunk alone would not always guarantee), CRC-32s and LDPC-encodes each
   chunk independently, and rate-matches each to the fixed `CODED_BLOCK_LEN=1944` coded bits, same
   as a single-codeword frame did for its one codeword. `codewords == 1` takes exactly one trip
   through this same code path with a single, whole-payload chunk -- byte-for-byte identical
   behavior to the pre-this-task codec.

3. **Intra-frame cross-codeword interleaving reuses `CrossFrameInterleaver` verbatim, re-scoped
   from across-frames to across-codewords-within-one-frame, gated to `level >= 5`.** The struct's
   permutation math is unchanged (`crates/coppa-codec/src/ofdm/cross_frame_interleaver.rs` has zero
   code changes from this task) -- only its caller changed: `CoppaTransceiver::transmit` calls
   `CrossFrameInterleaver::new(codewords, CODED_BLOCK_LEN).interleave(...)` across the `codewords`
   per-codeword coded-bit blocks of ONE frame, where it used to be called (only in a bench tool,
   never production) across N *separate* frames' codewords. Per decision 6's own reasoning (lower
   levels' symbol rate/coherence-time margins are already adequate without it), the gate is exactly
   `codewords > 1 && level >= 5` -- not applied at all below level 5, even for a multi-codeword
   frame there. A test/bench-only override (`with_cross_codeword_interleave_override`) exists to
   force the gate on/off independent of level, for a controlled A/B measurement of the interleaving
   effect alone (see the Task 5 report's scenario (d) results) -- production code never sets it.

4. **ACK addressing by `(seq, codeword-index)` is explicitly OUT OF SCOPE for this task.** Decision
   6's text describes it, but `ArqTx`/`ArqRx` (`crates/coppa-protocol/src/arq.rs`) remain entirely
   `seq: u8`-addressed, whole-segment ACK/retransmit, unchanged by this task. None of this task's 4
   failing-test scenarios exercise partial-frame retransmission, and building real per-codeword ARQ
   addressing would be a large, untested expansion touching Task 2's already-reviewed SACK/RTO work.
   A multi-codeword frame is retransmitted, if at all, as a whole (same `seq`, cycling RV via the
   existing `crate::arq::rv_for_attempt` mechanism) -- exactly like a single-codeword frame today.

5. **Neither Task 5(Phase 2)'s one-round turbo re-estimation nor Phase 3 Task 3's persistent
   IR-HARQ combining across retransmissions extend to multi-codeword frames.** A single-codeword
   frame (`codewords <= 1`) takes the exact pre-this-task decode path
   (`CoppaTransceiver::receive_single_codeword`), turbo and IR-HARQ included, completely unchanged.
   A multi-codeword frame takes a separate path (`receive_multi_codeword`) that decodes every
   codeword fresh from that one transmission's own LLRs, with no turbo retry and no persistent
   cross-retransmission LLR accumulator. This is a real, flagged scope decision, not an oversight --
   see the Task 5 report for the full reasoning (turbo's channel re-fit operates across the WHOLE
   frame's symbols from ONE codeword's posterior; generalizing "which codeword's posterior drives
   the re-fit, and do already-converged codewords get re-decoded too" is a real design question none
   of this task's 4 scenarios exercise; IR-HARQ combining has no defined per-codeword ARQ addressing
   to key its accumulator on, per decision 4 above).

6. **Whole-frame decode semantics: any one codeword failing (non-convergence OR its own CRC-32
   mismatch) fails the whole frame**, matching the existing whole-segment ARQ model exactly (a
   single-codeword frame already worked this way; a multi-codeword frame just has more chances to
   fail). A separate, additive diagnostic method,
   `CoppaTransceiver::receive_multi_codeword_diagnostic`, reports PARTIAL per-codeword success
   (which codewords individually converged + CRC-checked) for bench/test instrumentation that needs
   that visibility (e.g. measuring interleaving's diversity benefit, which a pass/fail whole-frame
   metric can't express) -- it does not change `receive`/`receive_with_metrics`'s own semantics.

## Consequences

- **Positive**: a 10 kB bulk transfer's estimated time (single-turnaround-per-frame model, see
  `crates/coppa-bench/examples/task5_multi_codeword_transfer.rs`) drops from ~57 s to ~29 s at level
  6 (target was <= 40 s) and ~133 s to ~106 s at level 2, purely from needing far fewer frames for
  the same payload. Scenario (d)'s measured result (30 seeded trials, 8 codewords at level 5, a 0.4 s
  mid-frame deep fade): interleaving ON recovers all 8 codewords every trial; OFF recovers only 6 on
  average -- a real, reproducible >= 2-codeword diversity win, matching the plan's own reasoning.
- **Negative / honestly re-derived acceptance figure**: the plan's own pre-implementation estimate
  for scenario (c)'s specific configuration (level 6, 7 codewords, 800 bytes) was "airtime <= 0.55x
  seven single-codeword frames' total airtime"; the real, measured, deterministic ratio is **~0.639**
  (a real ~36% airtime reduction, not the ~45% the 0.55 figure implied). Root cause: the "47% -> ~10%
  at 64-QAM" figure decision 6 quotes is calibrated for 64-QAM, where the fixed overhead is a much
  larger fraction of one codeword's (very short) payload span than it is at level 6's 16-QAM -- less
  headroom to amortize away at a lower-order modulation. Same pattern as this codebase's other
  honestly-re-derived acceptance figures (see `CLAUDE.md`'s Known Limitations, the soft-header
  gap re-derivation). The test (`multi_codeword_frame_airtime_beats_seven_single_codeword_frames`)
  asserts the real, reproducible bound (<= 0.65x) rather than a known-false 0.55x.
- **Negative / accepted trade-off, wire-format break**: a frame with `codewords > 1` is not
  decodable by the pre-this-task codec's fixed single-codeword symbol-count assumption
  (`CoppaModem::demodulate_frame`'s payload-symbol-count arithmetic now multiplies by
  `header.codewords`); `codewords == 1` remains byte-for-byte and bit-for-bit identical to every
  frame before this task, so old<->new interop is preserved for single-codeword traffic specifically
  -- the break only affects newly-multi-codeword traffic, which did not exist before this task. Same
  pattern as the Phase 1 waveform break (ADR-003) and the Phase 2 NR BG2 break (ADR-005).
- **Flagged, not silently dropped, gap**: `(seq, codeword-index)` ACK addressing (decision 4 above)
  and turbo/IR-HARQ extension to multi-codeword frames (decision 5 above) are real, deliberate scope
  cuts for a future task, not oversights -- see the Task 5 report (`.superpowers/sdd/task-5-report.md`)
  for the full reasoning behind each.
