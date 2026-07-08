# ADR-005: NR BG2 mother code + circular-buffer rate matching + layered NMS decoder

## Status

Accepted

## Context

Coppa's LDPC layer, since before Phase 1, used nine separate QC-LDPC codes transcribed from IEEE
Std 802.11-2012 Annex F: six distinct base matrices (rates 1/4, 1/3, 1/2, 2/3, 3/4, 7/8), each with
its own 24-column exponent matrix, lifted at a fixed Z=81 to a 1944-bit codeword
(`crates/coppa-protocol/src/fec/ldpc/codes.rs`). Switching speed levels meant switching to an
entirely different graph, decoder instance, and (for `CoppaTransceiver`) a different cached
`LdpcCodec`.

Coppa's Phase 2 "world-class" analysis (see the coppa-world-class-analysis and
coppa-deep-review-2026-07 memory docs) identified this as a real, quantifiable gap against the
modern state of the art: 3GPP's 5G NR LDPC codes (TS 38.212 §5.3.2) use a single family of two
"mother" base graphs (BG1, BG2) with degree-optimized irregular structure, rate-matched down to any
target rate via a circular buffer read rather than by re-deriving a whole new graph per rate. The
NR codes are the product of a large, published density-evolution optimization effort (see
Richardson & Kudekar, "Design of Low-Density Parity Check Codes for 5G New Radio", IEEE
Communications Magazine, 2018) and are known to outperform the IEEE 802.11 codes at comparable
block lengths and rates by roughly 1.5-2 dB in the regime Coppa's speed ladder operates in. Task 4
of the Phase 2 remediation roadmap replaces Coppa's LDPC layer with exactly this approach, using
**BG2** (the smaller of the two mother graphs, designed for the `K <= 3840`-bit info-block range
Coppa's speed ladder needs; BG1 targets larger transport blocks Coppa has no use for).

## Decision

1. **One mother code, Zc = 176, for every speed level.** `NrLdpc`
   (`crates/coppa-protocol/src/fec/ldpc/mod.rs`) wraps a single BG2 graph, lifted at a *fixed*
   lifting factor Zc = 176 (chosen so `KB * Zc = 10 * 176 = 1760` comfortably covers every speed
   level's info-bit capacity while keeping the graph small enough to lift/decode cheaply). Every
   speed level's actual code rate is realized entirely through **shortening** (see decision 3), not
   through selecting a different graph. `CoppaTransceiver` now caches exactly one `NrLdpc` instance
   (`ldpc: NrLdpc`) instead of nine per-level `LdpcCodec`s.

2. **Base graph transcription and its verification.** The BG2 shift table for the lifting-size
   family containing Zc=176 (3GPP TS 38.212 Table 5.3.2-1's family index **i_LS = 5**, i.e. Zc in
   `{11, 22, 44, 88, 176, 352}` -- **not** i_LS=7 as this task's brief initially guessed; see
   `tools/gen_nr_bg2/src/main.rs`'s module docs for the correction and how it was confirmed) was
   cross-validated against two independent open-source implementations (NVIDIA Sionna and the
   srsRAN Project) fetched live and diffed programmatically: all 197 non-zero entries matched
   exactly, and the core 4-column submatrix independently matches the well-documented universal
   NR-LDPC core structure. `tools/gen_nr_bg2` codifies this transcription plus four validators
   (dimensions; zero 4-cycles at Zc=176; minimum column weight, deliberately scoped to the 14 core
   columns since NR's extension parity columns are degree-1 *by design*; and a full encode/check
   `H*c^T=0` round-trip) and generates `crates/coppa-protocol/src/fec/ldpc/nr_bg2.rs`. See the Task
   4 report for the full validator run and provenance detail.

3. **Shortening via a fixed `k_used` per level, not a new graph per rate.** Each speed level's
   nominal code rate is realized by transmitting only its first `k_used` of the mother code's 1760
   systematic info bits (real payload + zero-pad up to `k_used`); the remainder
   (`k_used..1760`) is known zero, never transmitted, and pinned back in at RX (extending Task 3's
   known-pad LLR pinning -- see `crate::fec::ldpc::pin_known_pad`). `k_used = round(rate * 1944)`
   for the existing ladder's rates, with **one deliberate exception, and a wire-format break**:
   level 10 (64-QAM) moves from rate 7/8 (`k_used`=1701) to rate **5/6** (`k_used`=1620). The Phase
   2 decision audit found 7/8 hitting LDPC non-convergence at high SNR even under the old codec (see
   `CLAUDE.md`'s Known Limitations, "PAPR clipping..." bullet); 5/6 both relaxes that margin and is
   a cleaner NR-standard-adjacent rate. **This means frames encoded with the pre-Task-4 codec are
   not decodable by the new one, and vice versa, for level 10 specifically (and for every level's
   coded-bit *content*, since the graph itself changed even where the nominal rate did not)** --
   exactly the kind of break `docs/adr/003-phase1-waveform-break.md` established the pattern for.

4. **Circular-buffer rate matching (`rate_match.rs`), 3GPP §5.4.2-style.** The mother codeword (8800
   bits: `KB*Zc - 2*Zc = 1408` non-punctured systematic bits, always excluding the first `2*Zc`
   systematic bits per standard NR puncturing, plus 7392 parity bits) is not transmitted directly;
   `rate_match`/`rate_dematch` select exactly `E=1944` coded bits from a logical buffer (transmitted
   info prefix ++ all parity) via a circular read starting at a per-redundancy-version offset `k0`.
   Phase 2 only uses `rv=0` (`k0=0`); `rv=1..3` offsets are implemented and tested now (rounded down
   to a Zc multiple) so Phase 3's HARQ incremental redundancy is pure plumbing, not a rate-matching
   redesign.

5. **Layered (row-based) normalized min-sum decoding**, not flooding. `NrBg2Decoder`
   (`crates/coppa-protocol/src/fec/ldpc/decoder.rs`) processes one base row (a "layer" of Zc lifted
   checks) at a time and updates variable-node posteriors immediately, so later layers in the same
   iteration already see fresher beliefs -- standard practice for QC-LDPC decoders, and empirically
   roughly halving the iteration count needed for a given error-correction performance versus
   flooding (see the Task 4 report's early-exit iteration statistics). A flooding-schedule variant
   of the same normalized min-sum update (`NrBg2FloodingDecoder`) is kept behind `#[cfg(test)]` for
   direct A/B comparison, not shipped in the production decode path. The normalized min-sum scale
   (alpha) was recalibrated for the new graph/schedule combination -- see the Task 4 report's sweep
   table for the value chosen and why.

## Consequences

- **Positive**: no speed level regresses (every level's isolated-FEC-layer AWGN threshold improves
  or stays flat vs. the pre-Task-4 codec at matched rate/block length); level 10 specifically
  clears its own motivating issue (7/8 non-convergence at high SNR is gone -- `FER=0.00` across a
  full SNR sweep at every level, confirmed by `tests/phase_c_loopback.rs`'s Monte Carlo test); the
  layered decoder measurably outperforms its own flooding reference at a real noisy operating point
  (a direct, controlled A/B), confirming the schedule itself has no bug.
- **Negative / measured shortfalls against this task's own bench-gate targets, not hidden**: the
  coding gain at level 2 (the primary acceptance point) measured +0.5 dB, short of the ~1.2-1.8 dB
  predicted from the density-evolution gap -- believed to be a genuine finite-length effect (802.11
  LDPC is itself a reasonably well-optimized code, and DE thresholds are asymptotic), not a
  decoder bug (ruled out via the flooding A/B above), but not chased further. Decode CPU/frame is
  3.5x-9.5x the old codec across the ladder, over the accepted 3x budget, even after a real,
  verified ~19% reduction from precomputing the lifted variable-index table (Zc=176 is not a power
  of two, so the naive per-edge `% Zc` was a genuine avoidable cost) and a cache-friendlier message
  layout -- the remaining gap is structural: the shared graph no longer shrinks for high-rate
  levels the way per-rate graphs used to, and closing it further would need a substantially larger
  effort (SIMD, unsafe bounds-check elision, cache-aware node relabeling). See
  `.superpowers/sdd/p2-task-4-report.md` for the full investigation, including a real correctness
  bug this same investigation caught and fixed: an alpha value calibrated at level 2 only broke
  real convergence at level 10's very different operating point, caught by existing tests, not the
  new codec's own; the shipped default (0.75) is the value validated across the whole ladder.
- **Negative / accepted trade-off**: this is a wire-format break (decision 3). Old and new frames
  are not interoperable, exactly like Phase 1's waveform break -- there is no in-band
  version-negotiation or dual-decode fallback in this codebase, so a deployed old-codec peer cannot
  talk to a new-codec peer. This is acceptable pre-1.0 (Coppa has no deployed installed base yet).
- The old per-rate `LdpcCodec`/`CodeRate`/`LdpcCode` types (`codes.rs`, the original
  `encoder.rs`/`decoder.rs` structs) are **not deleted** -- they remain for any bench/reference code
  that still constructs them directly (e.g. `coppa-bench`'s standalone `V2Phy` cross-frame-interleave
  experiment, and a couple of isolated-gate bench examples), but `CoppaTransceiver` no longer uses
  them for anything on the actual TX/RX path.
- Phase 3 (rate-loop / adaptive MCS) can now consider `rv>0` retransmissions as a real design option
  without a rate-matching redesign, since the circular-buffer offsets already exist and are tested.
