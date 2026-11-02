# Coppa PHY Benchmarks

This file is the current-state summary. The dated measurement logs that used
to make up this file are in [`docs/benchmarks/`](docs/benchmarks/), one file
per log, unchanged; the [log index](#log-index) below maps every old section
title to its file.

## Scope

Every number here is an in-process simulation: the `coppa-bench` harness
drives `CoppaTransceiver` (or the engine or daemon, where a log says so)
through the channel models in `coppa-channel`. There is no on-air or
two-radio data anywhere in this file or in `docs/benchmarks/`.

## Conventions

- **SNR** is referenced to a 3 kHz noise bandwidth against clean signal
  power (`coppa_channel::awgn_ref_seeded`). Logs dated before the 2026-07
  [conventions correction](docs/benchmarks/2026-07-measurement-conventions-rebaseline.md)
  use a full-band SNR that reads about 9 dB optimistic, and are not
  comparable with the numbers below.
- **FER claims** in logs since the conventions correction need the Wilson
  95% upper bound, not the point estimate, to clear the target. The CI guard
  tests below assert point estimates instead, because 20 trials cannot gate
  FER ≤ 10% on a Wilson bound.
- **Seed:** the harness default base seed is `0x00C0FFEE` (`12648430`);
  trial `n` uses `seed + n`.
- **Level numbers** follow the COP-24 renumbering (levels 1-9; HF engines
  carry 1-8). Logs written before 2026-10-23 use the old numbers;
  [ADR-009](docs/adr/009-cop24-speed-ladder-renumber.md) has the mapping.

## Per-level AWGN waterfall

The lowest SNR band where FER reaches 10% or less on AWGN, measured
2026-10-28 at `3cbfd1e` through `run_scenario`, 40 trials per point. Every
level is pinned to `hf_standard`, which is deliberately not the production
routing (`select_profile` sends level 5 and up to `vhf_wide`). Goodput is
the 20-trial baseline at the upper SNR of the level's CI bracket. The CI
floor is the minimum goodput the guard test asserts at that SNR. Source:
the doc comment and `bracket(...)` calls in
[`crates/coppa-bench/src/runner.rs`](crates/coppa-bench/src/runner.rs).

| Level | FER ≤ 10% at (dB) | Goodput at upper SNR (bps) | CI floor (bps) | Guarded by |
|-------|-------------------|----------------------------|----------------|------------|
| 1 | 4-6 | 328 at 8 dB | 290 | `waterfall_bracket_level_1` |
| 2 | ~4 | 651 at 8 dB | 580 | `waterfall_bracket_level_2` |
| 3 | ~4 | 1189 at 8 dB | 1050 | `waterfall_bracket_level_3` |
| 4 | 4-6 | 1808 at 8 dB | 1600 | `waterfall_bracket_level_4` |
| 5 | 8-10 | 2189 at 12 dB | 1950 | `waterfall_bracket_level_5` |
| 6 | ~8 | 1877 at 12 dB | 1650 | `waterfall_bracket_level_6` |
| 7 | 10-12 | 2855 at 14 dB | 2550 | `waterfall_bracket_level_7` |
| 8 | 14-16 | 3210 at 18 dB | 2850 | `waterfall_bracket_level_8` |
| 9 | 18-22 | 4023 at 24 dB | 3600 | `waterfall_bracket_level_9` |

Each guard test runs 20 trials at a below-waterfall SNR (must fail at least
15/20) and at the upper SNR (at most 1/20 errors). A failure on either side
means the waterfall moved and the anchors need re-measuring.

## Headline results

- **HF ceiling through the SSB filter (COP-24, 2026-10-23):** every HF level
  1-8 had 0/50 frame errors at every tested SNR from 18 to 60 dB through the
  300-2700 Hz SSB filter; level 9 never cleared 10% FER, which is why HF
  stops at level 8.
  [Log](docs/benchmarks/2026-10-23-cop-24-hf-ceiling-ssb.md).
- **LDPC decode CPU (COP-6, 2026-08-03):** the NR BG2 decoder meets the ≤3x
  CPU budget against the old codec at five of nine levels; four levels sit
  at 3.3-4.3x (old level numbers).
  [Log](docs/benchmarks/2026-08-03-cop-6-ldpc-decode-cpu.md).
- **Connected-session throughput (COP-38, 2026-10-27):** stop-and-wait ARQ
  runs at 0.57-0.67x of the old fire-and-forget clean-link rate, simulated.
  [Log](docs/benchmarks/2026-10-27-cop-38-session-throughput.md).
- **`hf_narrow` level 1 below 0 dB (COP-67, 2026-10-28):** the weak-signal
  target failed. Every subzero point lost 200/200 frames, and +4 dB still
  has 16.5% FER. [Log](docs/benchmarks/2026-10-28-cop-67-hf-narrow-level-1.md).

## Unmet bars

These are tracked in [`docs/LIMITATIONS.md`](docs/LIMITATIONS.md), which is
the authority; see these bullets there:

- "Phase 3's closed-loop rate adaptation (Task 4) does not meet its own
  acceptance bar." (`RateLoop`)
- "The Phase 3 benchmark program (Task 8: `milstd`, `session`) does not clear
  its own acceptance targets"
- "Decision 4's block-ACK cadence (Phase 3 Task 2) was never implemented."
- "Phase 2's cumulative full-ladder re-baseline (Task 8) does not cleanly
  clear the phase's own acceptance bar."

## Reproduce

```sh
# Full FER/goodput sweep (CSV in results/, gitignored)
cargo run -p coppa-bench --release -- --trials 50
# A specific measurement program named in a log
cargo run -p coppa-bench --release --example <name>
# The per-level CI guard behind the table above
cargo test -p coppa-bench --lib waterfall_bracket
```

## Log index

Every section title of the pre-COP-88 `BENCHMARKS.md`, in its original order
(newest first, then the undated baseline, then later appends).

| Old section title | File | Date |
|---|---|---|
| COP-24: 64-QAM 5/6 dropped from HF; ladder renumbered 1-9 | [2026-10-23-cop-24-hf-ceiling-ssb.md](docs/benchmarks/2026-10-23-cop-24-hf-ceiling-ssb.md) | 2026-10-23 |
| 2026-07-28 — RateLoop capacity-metric same-frame accuracy: ground-truthed, root-caused as a | [2026-07-28-rateloop-capacity-ground-truth.md](docs/benchmarks/2026-07-28-rateloop-capacity-ground-truth.md) | 2026-07-28 |
| 2026-07-26 — SPEED_LEVEL_MIN_CAPACITY recalibration from clean (post-HARQ-fix) data | [2026-07-26-speed-level-min-capacity-recalibration.md](docs/benchmarks/2026-07-26-speed-level-min-capacity-recalibration.md) | 2026-07-26 |
| 2026-07 — RateLoop capacity-metric level-bias diagnosis | [2026-07-rateloop-level-bias-diagnosis.md](docs/benchmarks/2026-07-rateloop-level-bias-diagnosis.md) | 2026-07 |
| 2026-07 — RateLoop capacity-metric level-bias: root cause isolated to per-level PAPR clip targets | [2026-07-rateloop-level-bias-papr-root-cause.md](docs/benchmarks/2026-07-rateloop-level-bias-papr-root-cause.md) | 2026-07 |
| 2026-07 — RateLoop capacity/selectivity level-bias correction | [2026-07-rateloop-level-bias-correction.md](docs/benchmarks/2026-07-rateloop-level-bias-correction.md) | 2026-07 |
| 2026-07 — Short-CP coherence-time lever: measured against real fading | [2026-07-short-cp-coherence-time.md](docs/benchmarks/2026-07-short-cp-coherence-time.md) | 2026-07 |
| 2026-07 — Fading root-cause investigation: the coarse-delay-reference theory falsified | [2026-07-fading-coarse-delay-falsified.md](docs/benchmarks/2026-07-fading-coarse-delay-falsified.md) | 2026-07 |
| 2026-07 — Cascaded coarse-delay drift + AR(1) tap tracker (measured, gate NOT met, shipped disabled — third and final attempt in this line) | [2026-07-drift-cascade-ar1.md](docs/benchmarks/2026-07-drift-cascade-ar1.md) | 2026-07 |
| 2026-07 — Coarse-delay drift Kalman tracker (Task 5: measured, gate NOT met, shipped disabled) | [2026-07-drift-kalman-tracker.md](docs/benchmarks/2026-07-drift-kalman-tracker.md) | 2026-07 |
| 2026-07 — Phase 4 (field readiness): what changed vs. the 2026-07-03 world-class analysis | [2026-07-phase4-field-readiness.md](docs/benchmarks/2026-07-phase4-field-readiness.md) | 2026-07 |
| 2026-07 — Phase 3 Task 8: MIL-STD ladder, session-robustness bench, golden vectors | [2026-07-phase3-milstd-session-golden.md](docs/benchmarks/2026-07-phase3-milstd-session-golden.md) | 2026-07 |
| 2026-07 — Phase 3 Task 4: closed-loop adaptive rate | [2026-07-phase3-closed-loop-rate.md](docs/benchmarks/2026-07-phase3-closed-loop-rate.md) | 2026-07 |
| 2026-07 — Phase 2 (parametric estimation + NR BG2): cumulative re-baseline | [2026-07-phase2-cumulative-rebaseline.md](docs/benchmarks/2026-07-phase2-cumulative-rebaseline.md) | 2026-07 |
| 2026-07 — Measurement conventions corrected (read before older sections) | [2026-07-measurement-conventions-rebaseline.md](docs/benchmarks/2026-07-measurement-conventions-rebaseline.md) | 2026-07 |
| 2026-07 — Interleaver puncture + LDPC clamp hotfix | [2026-07-interleaver-puncture-hotfix.md](docs/benchmarks/2026-07-interleaver-puncture-hotfix.md) | 2026-07 |
| 2026-07 — Radio-reality Phase 1: carrier offset + Newman preamble + TX conditioning | [2026-07-phase1-radio-reality.md](docs/benchmarks/2026-07-phase1-radio-reality.md) | 2026-07 |
| 2026-07 — Phase 1 (radio reality): final acceptance sweep | [2026-07-phase1-final-acceptance.md](docs/benchmarks/2026-07-phase1-final-acceptance.md) | 2026-07 |
| 2026-07 — Hotfix: sparse-pilot header Watterson-fading regression (ADR-004) | [2026-07-sparse-pilot-header-hotfix.md](docs/benchmarks/2026-07-sparse-pilot-header-hotfix.md) | 2026-07 |
| Current performance (all fixes applied) | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| AWGN channel | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| Thesis check: do the higher-order modes buy more goodput at high SNR? | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| Watterson HF fading channels | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| Transfer-level cross-frame interleaving: a falsified diversity hypothesis | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| Root cause of the fading collapse: an LDPC decoder scale bug (fixed) | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| Adaptive MCS (capacity selector) + the noise-estimate fix that unblocked it | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| The frame header was the dominant fading failure — now protected (Golay + CRC + 2D) | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| Limitations | [undated-pre-phase1-baseline-and-fading-fixes.md](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) | undated (pre-Phase 1) |
| COP-6 LDPC decode CPU | [2026-08-03-cop-6-ldpc-decode-cpu.md](docs/benchmarks/2026-08-03-cop-6-ldpc-decode-cpu.md) | 2026-08-03 |
| Sample-clock offset over session-length transmissions (COP-3) | [2026-08-03-cop-3-sample-clock-offset.md](docs/benchmarks/2026-08-03-cop-3-sample-clock-offset.md) | 2026-08-03 |
| Level 9 Watterson fading diagnosis (COP-4) | [2026-08-05-cop-4-level9-watterson-fading.md](docs/benchmarks/2026-08-05-cop-4-level9-watterson-fading.md) | 2026-08-05 |
| Short-CP under closed-loop rate adaptation (COP-2) | [2026-08-05-cop-2-short-cp-closed-loop.md](docs/benchmarks/2026-08-05-cop-2-short-cp-closed-loop.md) | 2026-08-05 |
| COP-38: connected-session stop-and-wait throughput (simulated) | [2026-10-27-cop-38-session-throughput.md](docs/benchmarks/2026-10-27-cop-38-session-throughput.md) | 2026-10-27 |
| COP-67: `hf_narrow` level-1 AWGN feasibility | [2026-10-28-cop-67-hf-narrow-level-1.md](docs/benchmarks/2026-10-28-cop-67-hf-narrow-level-1.md) | 2026-10-28 |
