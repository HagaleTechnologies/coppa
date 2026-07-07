# Coppa world-class gap analysis — 2026-07-03

Synthesis of four parallel deep audits (coding theory, estimation theory, efficiency +
real-world utility — all against branch `feature/measurement-truth`) plus a 106-agent
adversarially-verified research sweep of the HF-modem state of the art. Every quantitative
claim below was either derived against the checked-in source (with independent verification
of the headline items) or is cited to a primary source. Clean-room note: all Mercury
information is from public announcements/README only; no Mercury source was read.

---

## 0. The scoreboard: what "world-class" objectively means

Two verified external anchors define the target:

1. **The 2020 Winlink IONOS simulator study** (the only rigorous public head-to-head;
   winlink.org, Nov 2 2020). Its central lesson, stated against the authors' own interest
   (they wrote ARDOP/WINMOR): **PACTOR 4 and VARA never dropped a connection under any
   condition; the open-source modems suffered frequent drops and throughput collapse on
   multipath.** Raw throughput was secondary — VARA only beats PACTOR 4 at very high SNR and
   *loses* to PACTOR 2 below ~14 dB in 500 Hz. → **Never-drop link robustness is the headline
   competitive dimension, not peak bps.**
2. **MIL-STD-188-110D Table XVI / C-XVII** gives an objective SNR-vs-rate ladder on the
   standard Watterson channels: BER 1e-5 at **18 dB SNR for 2400 bps** on Poor (2 ms/1 Hz),
   down to **2 dB for 75 bps** (5 ms/5 Hz); Appendix C QAM: 14/19/23/27/31 dB for
   3200–9600 bps on Poor. Notably, even Appendix D (wideband, 2017+) still uses
   **convolutional** FEC — a well-executed LDPC design can beat the mil standard's coding.
   → Adopt Table XVI as coppa's published benchmark scoreboard (the Phase 0 harness can now
   measure it honestly).

Peer context: Mercury (Rhizomatica) = FreeDV data modes + custom HERMES ARQ with hybrid
SNR+delivery-feedback gear-shifting and per-direction mode selection; claims VARA parity at
good SNR / superiority at poor SNR, **with no published numbers** — so a coppa that publishes
honest Table-XVI/IONOS-style results with CIs would be the only open-source modem with
credible public benchmarks. STANAG 4538's code-combining HARQ (soft-combining retransmissions)
maintains ARQ efficiency from **−6 to +20 dB**, and its HDL+ variant nets ~10 kbps in 3 kHz —
the direct model for coppa's ARQ upgrade path.

---

## 1. CONFIRMED BUG (fix immediately, before any Phase-2 benchmarking)

**`BlockInterleaver` structurally punctures 35 coded bits of every frame on `hf_standard`**
(`crates/coppa-codec/src/ofdm/interleaver.rs:18-51`). With block_size=1944, carriers=44, the
45×44 grid has 36 pad cells that land mid-scan in the column-major readout: the read loop
emits 35 pad zeros and **never transmits the last 35 real coded bits** (indices ≡ 43 mod 44);
on RX the same 35 positions stay 0.0 LLR → permanent erasures. Independently verified by
direct simulation of the loop logic.

- Impact: fixed 1.8% self-puncture = 2.4% of parity at R=1/4 … **14.4% of parity at R=7/8**
  (which consumes ~37% of that code's entire erasure margin, ε*=4.9%). ≈0.1 dB at R=1/2;
  ≈0.5+ dB and floor behavior at R=3/4–7/8.
- Both existing tests pass **by coincidence**: all 35 dropped indices are odd; the roundtrip
  test uses an `i%2` pattern and 0.0-LLR→bit-1 mapping that exactly matches every dropped bit.
- `hf_robust` (54×36=1944 exact) is unaffected — silently confounding any profile A/B bench.
- Fix: make the grid mapping a true bijection over 1944 elements (skip cells with
  `idx >= n` on both sides); replace the coincidence-passing tests with a random-pattern
  bijection test.

## 2. The big dB levers (ranked, gain × feasibility, beyond the existing Phases 1–3)

| # | Item | Gain | Cost | Source audit |
|---|------|------|------|--------------|
| 1 | **First-path timing + FFT window back-off** — xcorr refinement currently locks the *strongest* path; on the equal-power 2-tap channel that's the late tap 50% of the time → window slides 96 samples into the next symbol → −10 dB self-ISI ceiling on ~half of dispersive frames | removes a −10 dB ceiling on ~50% of Poor/Moderate frames | ~5 lines | estimation §1.2 |
| 2 | **Delay-domain parametric channel estimation** — linear frequency interpolation is the receiver's single largest loss (~2 dB Moderate, ~8.5 dB effective floor on Poor); the pooled pilot comb already satisfies Nyquist — fit the 2–3 physical delay taps directly (LS on delay grid, support seeded by the repurposed fine-sync probe) | **+2 dB Moderate, +5–8 dB Poor**, at zero pilot cost; makes `hf_robust`'s 18% carrier sacrifice unnecessary; fixes edge-carrier extrapolation for free | moderate (one L×L complex solve) | estimation §2.1–2.2 |
| 3 | **Kalman/fixed-lag-smoother channel tracker** (productionized form of #2): state = delay taps, AR(1) matched to the Gaussian Doppler; even/odd pilot alternation becomes a time-varying observation matrix the filter exploits optimally; innovation gives honest per-carrier σ² | statistically optimal 2D estimation, *cheaper* than today's per-symbol re-solve | moderate | estimation §2.5 |
| 4 | **One-round LDPC-aided turbo re-estimation** — on decode failure, use soft symbol means as 44 weighted virtual pilots, re-fit, re-demap, decode again. (Research sweep corroborates: iterative equalization ≈4 dB class gains on doubly-dispersive channels; in coppa's CP-sufficient/ICI-free regime, turbo *estimation* is the right variant, not turbo equalization.) | +1.5–2.5 dB at fading FER=10% points | 1 extra LDPC pass on demand | estimation §3.1 + research [7] |
| 5 | **Replace LDPC matrices with 5G NR BG2 (low rates) + 802.11n (mid/high) and adopt IR-HARQ** — current matrices are ~1.8 dB (R=1/2) to ~2.9 dB (R=1/4) off their claimed pedigree, every rate has 81 degree-1 variable nodes (open accumulator chain) inflating near-threshold FER; NR BG2's native 1/5 mother rate gives a redundancy-version ladder so retransmissions carry *new parity* (IR) instead of repeats (Chase): +0.5–2.5 dB per round beyond Chase, matching the STANAG 4538 code-combining model. Needs: 2-bit RV index (fits the constant `fec_type` header field), LLR buffer already planned for Chase | ~1.8 dB first-tx at R=1/2; IR ladder converts deep fades into descending-rate retransmission | medium-high (two-step encoder prerequisite already known) | coding §A2, §(c) + research [5][6] |
| 6 | **Soft-ML Golay header + CRC-assisted list** — full 4096-codeword correlation ML (~0.6 M MACs/header, trivially SIMD-able) is exactly ML: +1.9 dB AWGN, more in fading (erasure budget 3→7/word ≈ doubles tolerable notch width); then try top-2 candidates per word against CRC-16 (≤64 checks) for another ~0.3–0.5 dB | ~+2.2–2.4 dB on the dominant fading failure | low-moderate | coding §A6 |
| 7 | **Two-stage CFO from the intrinsic lag-480 periodicity** — the even-bin preamble is periodic at N/2, so Moose at lag 480 gives **±50 Hz pull-in** (exactly the SSB spec) to disambiguate the near-CRLB lag-1260 estimator (σ≈0.2 Hz @ 10 dB); simpler than the planned frequency-domain integer-comb search, zero waveform change | ±19 Hz → ±50 Hz acquisition for free | low | estimation §1.1 |
| 8 | **Newman-phase (CAZAC-class) in-band preamble** — current preamble is Gaussian-envelope BPSK-PN (PAPR ≈11 dB) clipped to near-square at −1 dB vs own RMS; Newman phases give PAPR ≈2.5–3 dB → **+7–9 dB preamble energy at fixed peak**, on top of +9.8 dB from in-band placement (only ~25 of 240 comb lines survive a 300–2700 Hz filter today). Keep two-identical-halves + even bins so #7 and S-C survive | +7–9 dB detection/CFO energy | low-moderate | estimation §1.3 |
| 9 | **Layered LDPC scheduling + message clamping + degree-matched α** — flooding under the 50-iteration cap loses ~0.2–0.4 dB near threshold; unclamped f32 messages can reach inf→NaN on non-convergent frames (NaN poisons min-scan and hard decisions) | ~0.2–0.4 dB effective + 2× average decode speed + removes NaN edge | small | coding §A4 |
| 10 | **Short-CP profile (3 ms)** gated on probe-measured delay spread — mid-latitude 95th-pct composite multipath ≤3 ms; prerequisites are #1 and SCO tracking | +11% throughput, +0.55 dB | small, after #1 | estimation §3.2 |

Cross-audit convergence worth noting: the coding audit independently concluded that **no
within-frame interleaver can save a codeword from one coherence-time fade** (a 1 Hz-spread
fade erases 33% of a 45-symbol BPSK codeword; R≥2/3 is unrecoverable) — hard numbers behind
the already-planned cross-frame interleaver, which dilutes the same fade to ~4%/codeword at
N=8. And the estimation audit's "fix CSI before turbo" ordering matches the coding audit's
"fix matrices before α-tuning": in both chains, land the model fix before the refinement.

## 3. Efficiency (measured, Apple M4, release)

- **Sync scan is 97% of all RX CPU** (52.8 of 54.3 ms/frame; 0.040× realtime always-on).
  Beyond the planned O(1) sliding metric: **stride the coarse search by ~150 samples** (the
  S-C plateau is CP-wide; the existing ±CP xcorr refinement recovers exact timing) — ~100×
  fewer metric evaluations, ~10 lines, multiplicative with the O(1) rewrite.
- **`LdpcCodec::new` is rebuilt on every transmit AND receive** (`transceiver.rs:54,125`):
  0.105 ms, 4801 allocs, 525 KB each — 54% of all RX allocations. Cache per speed level.
- Demod allocates ~112 heap Vecs per OFDM symbol (BTreeMap pooling, per-symbol estimator,
  per-symbol demap Vecs) — 8951 allocs/6.3 MB per SL1 frame. Fixed workspaces + `forward_into`
  FFT API zero this out; this is the embedded-jitter risk, not throughput.
- 16/64-QAM demappers re-enumerate the constellation per bit (384 distance evals/symbol);
  Gray-QAM factorizes into per-axis PAM with closed-form piecewise-linear LLRs — 8–24×
  cheaper, bit-exact.
- **Embedded verdict**: Pi Zero 2/A53 feasible today (sync ≈0.4–0.8× realtime/core), trivial
  after the sync fixes; f32+NEON SIMD is the right investment, fixed-point buys nothing on
  A53-class.

## 4. Real-world utility gaps (what actually blocks field adoption)

Ranked by operator impact; the research sweep confirms these — not dB — are what separated
ARDOP-class from VARA-class in the only public benchmark:

1. **No telemetry ever reaches a client**: VARA-port `SNR`/`BUSY`/`PTT`/`BUFFER` responses
   exist as types but are never emitted; WebSocket `status` hardcodes `connected:false`.
   A Winlink-style host cannot function — this blocks the primary use case.
2. **No TX level calibration** (no tune/two-tone command) — first thing a real operator needs.
3. PTT stubs (`serial`/`gpio` fall back to `NullPtt` silently); CLI hardcodes rigctld address
   and lead-in times.
4. **No busy-channel detection** (ironically `spectrum_sensor.rs` has the pieces, unused);
   no station ID timer (US regulatory blocker for unattended ops); no beacon/CQ mode; no
   waterfall data over any interface; `coppa rx` prints "not yet implemented".
5. **Reference-implementation credibility gaps**: zero golden test vectors (no WAVs anywhere),
   no waveform conformance spec (a second implementation cannot be written from the docs),
   no OTA methodology. For the project's stated purpose these may outrank dB items.
6. **FFI is not integrable**: text-only API, no binary payloads, no config surface, no
   metadata, stale 8 kHz streaming constants.
7. **Protocol arithmetic**: at speed 6, ~half of theoretical throughput evaporates into
   per-frame overhead + full-codeword ACKs (10 kB file: 61 s ≈ 1350 bps vs 3352 bps
   instantaneous). Multi-codeword frames + block-ACK (planned) fix this; a lost ACK today
   stalls 8% of a whole transfer (RTO 5 s).

## 5. Research bets — explicit verdicts

**Worth doing** (in order): delay-domain/Kalman estimation (§2), IR-HARQ on NR BG2 (§5),
turbo re-estimation (§4), soft-ML Golay + CRC list (§6), probabilistic amplitude shaping at
levels 7–10 later (+0.4–0.8 dB, continuous rate granularity), NB-LDPC over F256 as the
research-grade option for a future control channel (strongest known short code: 0.7 dB from
the normal approximation, no floor to CER 1e-9 — but a wire-format break, so only with a
version bump).

**Explicitly not worth it, with reasons** (so we don't relitigate): OTFS/OCDM (normalized
Doppler 0.026, ICI −40 dB — OTFS pays only when Doppler approaches subcarrier spacing;
revisit only for an auroral 10–50 Hz profile); EP/turbo *equalizers* proper (degenerate to
per-carrier MMSE in this CP-sufficient regime — the gain lives in estimation, not
equalization); superimposed pilots (recovers only the 8% pilot overhead at high complexity);
BICM-ID (Gray labeling makes the demapper EXIT curve flat, ≲0.1 dB); S-random within-codeword
interleaving (row-column is already maximal-product-spread for separable fades); neural
receivers (feasible to *run* in Rust, but without a training pipeline+dataset the project
can't sustain it, and the model-based sparse estimator captures most of the demonstrated
gain because it exploits the same structure they learn). **One design-doc-worthy research
direction**: SBL/SAGE joint delay-Doppler mode tracking — the exact statistical match to
Watterson physics; would subsume the estimator/tracker/CFO items into one factor-graph
receiver. **FT8/WSPR lessons** → a future low-rate ALE/beacon mode (long coherent integration,
strong sync sequences, ~50-bit payloads at −20 dB class SNR) is the right borrow, as a
separate waveform, not a modification of the data waveform.

## 6. Benchmark program (makes "world-class" falsifiable)

1. Add MIL-STD-188-110D Table XVI operating points to coppa-bench as named scenarios
   (2400 bps @ 18 dB Poor target, etc.) and publish results with Wilson CIs in BENCHMARKS.md.
2. Add an IONOS-style **session-robustness bench**: long ARQ sessions across the preset
   ladder scoring *connection survival* and net bytes/min — the metric that actually decided
   the 2020 comparison. Target: zero drops across all presets at any SNR where connect
   succeeds.
3. Golden test-vector corpus: freeze ~20 WAVs (frames at known SNR/channel/seed) + expected
   payloads + manifest; CI-check decodability. Doubles as the interop story.
4. Waveform conformance spec in docs/ (subcarrier map, pilot pattern, preamble, header
   format, matrices, interleaver, scrambler) — so a second implementation is possible.

## 7. Suggested sequencing impact on the existing roadmap

- **Hotfix now (pre-Phase-1)**: interleaver puncture bug (§1) + decoder message clamp
  (NaN edge) — both are correctness, both tiny.
- **Phase 1 additions**: first-path timing (#1, do *before* estimator work or its gain is
  understated), lag-480 CFO (#7, replaces the planned integer-comb search), sync stride +
  `LdpcCodec` caching (§3), Newman-phase preamble (#8, while the preamble is being made
  in-band anyway).
- **Phase 2 revisions**: delay-domain estimation (#2) *replaces* the planned `hf_robust`
  pilot-densification path; Kalman tracker (#3) and turbo re-estimation (#4) become the
  estimator's production form; soft Golay upgraded to full-ML + CRC list (#6); matrices item
  upgraded to NR BG2 + IR-HARQ target (#5); LLR scale fixes remain (they become load-bearing
  exactly when combining starts).
- **Phase 3 additions**: telemetry emission (§4.1) joins the rate-loop work (same code
  paths); IR-HARQ RV plumbing joins the Chase buffer item; benchmark program (§6) lands with
  the re-baselines.
- **New Phase 4 candidate (field readiness)**: TX calibration, PTT completion, busy detect,
  station ID, golden vectors, conformance spec, FFI v2 — the §4 list, none of which is
  gated on PHY work.

## Sources (research findings; all 3-0 adversarially verified unless noted)

- Winlink IONOS comparison, Nov 2 2020 (primary PDF, winlink.org) — modem head-to-head, never-drop finding.
- MIL-STD-188-110D (everyspec) — Table XVI/C-XVII ladders; App D convolutional-only FEC.
- STANAG 4538/4539 HARQ: IET IRST 2012 (10.1049/cp.2012.0367); Harris IET 2003 (10.1049/cp:20030430) — code combining −6..+20 dB; HDL+ ~10 kbps.
- Coskun et al., Physical Communication 2019 (arXiv:1812.08562) — short-blocklength survey: NR BG2 vs BG1 vs protographs; NB-LDPC F256.
- Li & Yu, IEEE Trans. Commun. 71(3) 2023 (arXiv:2207.00866) — iterative MMSE turbo equalization numbers (medium confidence, single paper).
- Rhizomatica Mercury announcement May 2026 + GitHub README (public only) — architecture and unquantified VARA-parity claim (medium confidence, vendor self-claim).
