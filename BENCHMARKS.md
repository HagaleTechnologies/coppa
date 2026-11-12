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

- **COP-129, 2026-11-08:** Golden coverage grew from 20 to 42 vectors; the additive generator preserves existing entries. [Log](docs/benchmarks/2026-11-08-cop-129-golden-vector-coverage.md).
- **COP-131, 2026-11-04:** Engine benchmarks separate local CPU latency from ideal clean-frame goodput; default routing uses VHF for levels 5-9. [Log](docs/benchmarks/2026-11-04-cop-131-engine-throughput.md).
- **COP-126, 2026-11-04:** The decimated sync pre-filter moved the full-band AWGN 50% sync point from about 12.3 dB to about 2.3 dB on standard HF levels 1-2. [Log](docs/benchmarks/2026-11-03-cop-126-decimated-sync-prefilter.md).
- **COP-119, 2026-11-06:** RS(12,6) session fade protection was measured with window-one MAC/transport arms and independent per-packet channels, including ACK loss; no RF trial. [Log](docs/benchmarks/2026-11-06-cop-119-rs12-6-session-fade-protection.md).
- **COP-133, 2026-11-04:** The same-frame residual candidate was not adopted; the rate-loop bar remains unmet. [Log](docs/benchmarks/2026-11-04-cop-133-same-frame-residual.md).
- **COP-132, 2026-11-04:** Synthetic impulse, clipping, AGC and tilt presets use independent-frame decoding, including the control; they are distinct from the legacy transfer baseline. [Log](docs/benchmarks/2026-11-05-cop-132-synthetic-transfer-impairments.md).
- **COP-152, 2026-11-06 merge date:** Bounded-prefix residual screening improved held-out scores, but production adoption was withdrawn after full-loop failure. [Log](docs/benchmarks/2026-11-06-cop-152-bounded-prefix-residual.md).

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

- "Multi-codeword frames (Phase 3 Task 5) do not extend ACK addressing, turbo re-estimation, or persistent IR-HARQ combining to the per-codeword level."
- "Phase 2 channel estimation (Task 1 delay-domain estimator + Task 7 Kalman tracker) — a real, unresolved regression, shipped anyway."
- "At level 3, an RV2 retransmission never rescues a failed RV0 frame (COP-129)."
- "Session payload compression has no store-raw fallback, so incompressible data grows on the wire."

## Reproduce

```sh
# Full FER/goodput sweep (CSV in results/, gitignored)
cargo run -p coppa-bench --release -- --trials 50
# A specific measurement program named in a log
cargo run -p coppa-bench --release --example <name>
# The per-level CI guard behind the table above
cargo test -p coppa-bench --lib waterfall_bracket
```

## COP-169: floor-mode measurement contract and the `ofdm:1` baseline

Measured 2026-11-11 on x86_64 Linux, release build, with the new
`floor_mode` diagnostic. The production floor waveform now exists (COP-190:
`coppa-codec::floor` and `coppa-protocol::modem::floor`, air-protocol
version 2); its development measurements are in the COP-190 section below.
These baseline rows remain the
`hf_narrow` level-1 frame (56-byte payload, 6.93 s airtime) that the floor
has to beat, measured the way the floor will be judged:

```
cargo run --release -p coppa-bench --example floor_mode -- sweep --channel awgn --trials 200 --out-dir results/cop-169/baseline-ofdm1-awgn
```

Each attempt runs the production batch receiver over a capture from the
persistent channel (`coppa_channel::persistent`): 0.35-0.40 s of noise, the
frame, 0.25 s of noise. Noise is set from the clean frame's power in 3 kHz.
Seeds are the development base `0x00C0FFEE + i`. `SyncFailed` is the
conservative count (no detection, mistiming or a failed header decode).
Raw rows and metadata: `results/cop-169/baseline-ofdm1-awgn/`.

AWGN, 200 attempts per point:

| SNR (dB) | Frame errors | `SyncFailed` | FER | 95% Wilson |
|---:|---:|---:|---:|---|
| −12 to 0 | 200 each | 200 each | 1.000 | 0.981-1.000 |
| +2 | 181 | 181 | 0.905 | 0.856-0.938 |
| +4 | 25 | 25 | 0.125 | 0.086-0.178 |

The 50% crossing is +3.04 dB against a −8 dB bar. Every failure is
`SyncFailed`, so the sync-share clause fails at +4 dB too. No wrong payload
was delivered. The verdict is FAIL, as expected for the baseline.

The matching Watterson Poor baseline (`--channel poor`) was started but not
run to completion in this session; it is the first measurement of the
follow-up, alongside the floor candidates.

## COP-169 experimental 16-FSK candidate

Measured 2026-11-11 with `floor_candidate`, release build, development seeds
`0x00C0FFEE`, ten attempts per point. These are prototype batch results,
not the 200-trial final sweeps or session acceptance evidence. Both channel
arms use the same full 40-byte random PDUs, active TX power and paired seeds.

```
cargo run --release -p coppa-bench --example floor_candidate -- --trials 10 --out-dir results/cop-169/dev-mfsk16-awgn
cargo run --release -p coppa-bench --example floor_candidate -- --channel poor --trials 10 --out-dir results/cop-169/dev-mfsk16-poor
```

The candidate uses 16 orthogonal tones from 600 to 1350 Hz at 50 baud,
a 16-symbol sync sequence, four repetitions of a CRC16-protected header,
and a fixed 40-byte PDU with CRC32, shortened NR BG2 coding and a time/bit
permutation. Airtime is exactly eight seconds, including acquisition and
header. All three current CONNECT PDU types fit the envelope in a clean
roundtrip test. The header's experimental value 2 does not allocate air
version 2 or change the production wire format. Metadata records the
constants and seed derivation.

| SNR (dB) | AWGN errors / 10 | Poor errors / 10 | Poor SyncFailed |
|---:|---:|---:|---:|
| -12 | 1 | 10 | 7 |
| -10 | 0 | 7 | 7 |
| -8 | 0 | 4 | 4 |
| -6 | 0 | 3 | 3 |
| -4 | 0 | 0 | 0 |
| -2 | 0 | 0 | 0 |
| 0 | 0 | 0 | 0 |
| 2 | 0 | 0 | 0 |
| 4 | 0 | 0 | 0 |

AWGN's crossing is at or below the grid's −12 dB lower edge. Poor's crossing
is −8.67 dB. Poor still FAILS the verdict because all failures at −8 and
−6 dB are header failures, conservatively counted as SyncFailed. No wrong
payloads were delivered. No waveform has been selected or frozen in an ADR.

Additional AWGN development sweeps used the same command with `--cfo 150`,
`--cfo -150`, and `--ssb`, writing respectively to
`results/cop-169/dev-mfsk16-cfo-plus150/`, `dev-mfsk16-cfo-minus150/` and
`dev-mfsk16-ssb/`. Both CFO arms lost all 90 frames with acquisition or header failure.
The receiver has no CFO search. The SSB-filtered arm lost 2/10 at −12 dB
and none at the other points; both losses were header failures, so its
conservative sync-share verdict also FAILS. These characterize limits;
they do not establish a radio operating envelope. All five sweeps saved
raw CSV and metadata. The white-noise rejection unit test covers one
8.02-second capture, not the planned 300-second multi-noise gate.

The batch receiver retains at most ten seconds of input and tries at most
16 separated acquisition candidates. `Capture` tests chunk partitioning
with bounded history, but decoding is explicitly requested after capture.
It is not a production streaming detector. Reported RX wall times cover
batch decode only. BPSK comparison, continuous streaming acquisition,
long noise-only runs, design selection, final seeds and production session
integration remain unbuilt. The ticket's three scenarios remain unmet.
This candidate was written using repository primitives and the ticket's
parameters. No external modem source was read or copied.

## Log index

Every section title of the pre-COP-146 file, in its original order. Grouped logs retain each section unchanged.

| Old section title | Date | File |
|---|---|---|
| COP-129: golden vectors for levels 3, 4, 7 and 9, multi-codeword frames and retransmission | 2026-11-08 | [log](docs/benchmarks/2026-11-08-cop-129-golden-vector-coverage.md) |
| Engine processing and ideal frame goodput (COP-131) | 2026-11-04 | [log](docs/benchmarks/2026-11-04-cop-131-engine-throughput.md) |
| COP-24: 64-QAM 5/6 dropped from HF; ladder renumbered 1-9 | 2026-10-23 | [log](docs/benchmarks/2026-10-23-cop-24-hf-ceiling-ssb.md) |
| 2026-07-28 — RateLoop capacity-metric same-frame accuracy: ground-truthed, root-caused as a fading-coherence ceiling (not a fixable estimator bug) | 2026-07-28 | [log](docs/benchmarks/2026-07-rateloop-capacity-metric.md) |
| 2026-07-26 — SPEED_LEVEL_MIN_CAPACITY recalibration from clean (post-HARQ-fix) data | 2026-07-26 | [log](docs/benchmarks/2026-07-rateloop-capacity-metric.md) |
| 2026-07 — RateLoop capacity-metric level-bias diagnosis | 2026-07 | [log](docs/benchmarks/2026-07-rateloop-capacity-metric.md) |
| 2026-07 — RateLoop capacity-metric level-bias: root cause isolated to per-level PAPR clip targets | 2026-07 | [log](docs/benchmarks/2026-07-rateloop-capacity-metric.md) |
| 2026-07 — RateLoop capacity/selectivity level-bias correction | 2026-07 | [log](docs/benchmarks/2026-07-rateloop-capacity-metric.md) |
| 2026-07 — Short-CP coherence-time lever: measured against real fading | 2026-07 | [log](docs/benchmarks/2026-07-short-cp-coherence-time.md) |
| 2026-07 — Fading root-cause investigation: the coarse-delay-reference theory falsified | 2026-07 | [log](docs/benchmarks/2026-07-fading-coarse-delay-falsified.md) |
| 2026-07 — Cascaded coarse-delay drift + AR(1) tap tracker (measured, gate NOT met, shipped disabled — third and final attempt in this line) | 2026-07 | [log](docs/benchmarks/2026-07-coarse-delay-drift-trackers.md) |
| 2026-07 — Coarse-delay drift Kalman tracker (Task 5: measured, gate NOT met, shipped disabled) | 2026-07 | [log](docs/benchmarks/2026-07-coarse-delay-drift-trackers.md) |
| 2026-07 — Phase 4 (field readiness): what changed vs. the 2026-07-03 world-class analysis | 2026-07 | [log](docs/benchmarks/2026-07-phase4-field-readiness.md) |
| 2026-07 — Phase 3 Task 8: MIL-STD ladder, session-robustness bench, golden vectors | 2026-07 | [log](docs/benchmarks/2026-07-phase3-milstd-session-golden.md) |
| 2026-07 — Phase 3 Task 4: closed-loop adaptive rate | 2026-07 | [log](docs/benchmarks/2026-07-phase3-closed-loop-rate.md) |
| 2026-07 — Phase 2 (parametric estimation + NR BG2): cumulative re-baseline | 2026-07 | [log](docs/benchmarks/2026-07-phase2-cumulative-rebaseline.md) |
| 2026-07 — Measurement conventions corrected (read before older sections) | 2026-07 | [log](docs/benchmarks/2026-07-measurement-conventions-rebaseline.md) |
| 2026-07 — Interleaver puncture + LDPC clamp hotfix | 2026-07 | [log](docs/benchmarks/2026-07-interleaver-puncture-hotfix.md) |
| 2026-07 — Radio-reality Phase 1: carrier offset + Newman preamble + TX conditioning | 2026-07 | [log](docs/benchmarks/2026-07-phase1-radio-reality.md) |
| 2026-07 — Phase 1 (radio reality): final acceptance sweep | 2026-07 | [log](docs/benchmarks/2026-07-phase1-radio-reality.md) |
| 2026-07 — Hotfix: sparse-pilot header Watterson-fading regression (ADR-004) | 2026-07 | [log](docs/benchmarks/2026-07-phase1-radio-reality.md) |
| Current performance (all fixes applied) | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| AWGN channel | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| Thesis check: do the higher-order modes buy more goodput at high SNR? | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| Watterson HF fading channels | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| Transfer-level cross-frame interleaving: a falsified diversity hypothesis | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| Root cause of the fading collapse: an LDPC decoder scale bug (fixed) | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| Adaptive MCS (capacity selector) + the noise-estimate fix that unblocked it | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| The frame header was the dominant fading failure — now protected (Golay + CRC + 2D) | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| Limitations | undated (pre-Phase 1) | [log](docs/benchmarks/undated-pre-phase1-baseline-and-fading-fixes.md) |
| COP-6 LDPC decode CPU | 2026-08-03 | [log](docs/benchmarks/2026-08-03-cop-6-ldpc-decode-cpu.md) |
| Sample-clock offset over session-length transmissions (COP-3) | 2026-08-03 | [log](docs/benchmarks/2026-08-03-cop-3-sample-clock-offset.md) |
| Level 9 Watterson fading diagnosis (COP-4) | 2026-08-05 | [log](docs/benchmarks/2026-08-05-cop-4-level9-watterson-fading.md) |
| Short-CP under closed-loop rate adaptation (COP-2) | 2026-08-05 | [log](docs/benchmarks/2026-08-05-cop-2-short-cp-closed-loop.md) |
| COP-38: connected-session stop-and-wait throughput (simulated) | 2026-10-27 | [log](docs/benchmarks/2026-10-27-cop-38-session-throughput.md) |
| COP-67: `hf_narrow` level-1 AWGN feasibility | 2026-10-28 | [log](docs/benchmarks/2026-10-28-cop-67-hf-narrow-level-1.md) |
| COP-126: decimated sync pre-filter on the streaming receive path | 2026-11-04 | [log](docs/benchmarks/2026-11-03-cop-126-decimated-sync-prefilter.md) |
| COP-119: RS(12,6) session fade protection | 2026-11-06 | [log](docs/benchmarks/2026-11-06-cop-119-rs12-6-session-fade-protection.md) |
| COP-133: Same-frame residual experiment | 2026-11-04 | [log](docs/benchmarks/2026-11-04-cop-133-same-frame-residual.md) |
| Synthetic transfer impairments (COP-132) | 2026-11-04 | [log](docs/benchmarks/2026-11-05-cop-132-synthetic-transfer-impairments.md) |
| COP-152: Bounded-prefix payload residual | 2026-11-06 (merge date) | [log](docs/benchmarks/2026-11-06-cop-152-bounded-prefix-residual.md) |

## COP-168 PHY HARQ schedule

This is a simulated PHY comparison, not an on-air throughput result. COP-184
keeps the COP-168 protocol and PHY work in this PR; daemon windowing and the
session throughput tables remain follow-ups. Existing coppad sessions still
send RV0 and do not use this schedule.

Built with `cargo build --release -p coppa-bench --example session_harq --offline`,
then run as `target/release/examples/session_harq`. These measurements use the
COP-168 working tree above base `1ac3273ed92f4ce36ce02e01102c0e1890cd9b4c`.
The diagnostic uses real HF transceivers, random session-framed payloads,
Watterson Moderate, fresh paired channel draws per physical attempt,
`awgn_ref_seeded` with a 3 kHz noise reference, and a 1200-sample receive tail.
The full output is retained in the COP-168 implementation artifact
`session-harq.txt`.

Calibration searches the 0.5 dB lattice from -10 to 50 dB with 400 independent
frames at each sampled point. Levels 1–3 reach the requested 25–35% first-frame
failure range. Levels 4–8 do not reach it in this search; their fading outage
floor stays above 35%. The table reports the closest measured point rather
than changing seeds or payloads to force the target.

For each level, the paired comparison stops at 400 failed RV0 frames or 2000
trials. Each failed frame is retried up to three times with Chase RV0 repeats
and all six permutations of RV1/RV2/RV3. Counts after attempts 2–4 are cumulative
rescues among failed initial frames. Intervals are 95% Wilson intervals.
Discordant pairs list schedule-only / Chase-only rescues. Selection maximizes
first-retry rescues, then within one frame of that maximum uses second-retry
rescues, then the smallest Kendall distance to 3GPP's RV2/RV3/RV1 order.
Level 9 copies level 8 because this run covers only HF levels 1–8. The
8-codeword level-4 diagnostic uses the single-codeword calibration SNR and
reports its own observed failure rate.

Release diagnostic wall time: 1344.912 s, or 22 min 24.9 s.

| Level | Codewords | SNR dB | Calibration RV0 failures / 400 | Main trials | Main RV0 failures | RV order after RV0 | First-retry margin vs Chase, pp |
|---|---|---|---|---|---|---|---|
| 1 | 1 | 6 | 119 | 1195 | 400 | 3 / 1 / 2 | -0.25 |
| 2 | 1 | 6 | 138 | 1118 | 400 | 3 / 1 / 2 | -6.75 |
| 3 | 1 | 16 | 102 | 1799 | 400 | 3 / 2 / 1 | +4.50 |
| 4 | 1 | 42.5 | 171 | 1025 | 400 | 3 / 2 / 1 | +13.25 |
| 4 | 8 | 42.5 | not separately calibrated | 402 | 400 | 3 / 2 / 1 | +3.00 |
| 5 | 1 | 46.5 | 275 | 635 | 400 | 3 / 2 / 1 | +18.25 |
| 6 | 1 | 42.5 | 220 | 694 | 400 | 1 / 2 / 3 | +29.50 |
| 7 | 1 | 35 | 307 | 491 | 400 | 3 / 2 / 1 | +28.25 |
| 8 | 1 | 35 | 369 | 425 | 400 | 3 / 2 / 1 | +24.75 |

Rescues below are cumulative counts among the 400 failed initial frames.
Bracketed intervals are percentages, not counts.

| Level | Codewords | Attempt | Chase rescues, Wilson 95% | Selected schedule rescues, Wilson 95% | Discordant schedule-only / Chase-only |
|---|---|---|---|---|---|
| 1 | 1 | 2 | 279 [65.08%, 74.05%] | 278 [64.82%, 73.81%] | 10 / 11 |
| 1 | 1 | 3 | 363 [87.51%, 93.21%] | 361 [86.95%, 92.79%] | 4 / 6 |
| 1 | 1 | 4 | 393 [96.43%, 99.15%] | 388 [94.83%, 98.28%] | 0 / 5 |
| 2 | 1 | 2 | 257 [59.44%, 68.79%] | 230 [52.61%, 62.25%] | 5 / 32 |
| 2 | 1 | 3 | 355 [85.28%, 91.49%] | 342 [81.71%, 88.61%] | 4 / 17 |
| 2 | 1 | 4 | 390 [95.46%, 98.64%] | 363 [87.51%, 93.21%] | 0 / 27 |
| 3 | 1 | 2 | 359 [86.39%, 92.35%] | 377 [91.52%, 96.14%] | 21 / 3 |
| 3 | 1 | 3 | 393 [96.43%, 99.15%] | 398 [98.20%, 99.86%] | 5 / 0 |
| 3 | 1 | 4 | 399 [98.60%, 99.96%] | 399 [98.60%, 99.96%] | 0 / 0 |
| 4 | 1 | 2 | 325 [77.13%, 84.77%] | 378 [91.81%, 96.34%] | 55 / 2 |
| 4 | 1 | 3 | 386 [94.21%, 97.90%] | 396 [97.46%, 99.61%] | 12 / 2 |
| 4 | 1 | 4 | 397 [97.82%, 99.74%] | 396 [97.46%, 99.61%] | 3 / 4 |
| 4 | 8 | 2 | 8 [1.02%, 3.90%] | 20 [3.26%, 7.60%] | 13 / 1 |
| 4 | 8 | 3 | 23 [3.86%, 8.48%] | 54 [10.50%, 17.20%] | 37 / 6 |
| 4 | 8 | 4 | 37 [6.79%, 12.49%] | 74 [15.00%, 22.60%] | 49 / 12 |
| 5 | 1 | 2 | 228 [52.10%, 61.76%] | 301 [70.79%, 79.23%] | 74 / 1 |
| 5 | 1 | 3 | 337 [80.36%, 87.49%] | 388 [94.83%, 98.28%] | 53 / 2 |
| 5 | 1 | 4 | 374 [90.65%, 95.53%] | 398 [98.20%, 99.86%] | 26 / 2 |
| 6 | 1 | 2 | 203 [45.87%, 55.62%] | 321 [76.07%, 83.86%] | 119 / 1 |
| 6 | 1 | 3 | 322 [76.33%, 84.08%] | 398 [98.20%, 99.86%] | 76 / 0 |
| 6 | 1 | 4 | 372 [90.07%, 95.11%] | 400 [99.05%, 100.00%] | 28 / 0 |
| 7 | 1 | 2 | 114 [24.30%, 33.11%] | 227 [51.85%, 61.52%] | 113 / 0 |
| 7 | 1 | 3 | 226 [51.60%, 61.27%] | 371 [89.78%, 94.90%] | 147 / 2 |
| 7 | 1 | 4 | 287 [67.15%, 75.94%] | 392 [96.10%, 98.98%] | 108 / 3 |
| 8 | 1 | 2 | 34 [6.15%, 11.64%] | 133 [28.81%, 38.01%] | 99 / 0 |
| 8 | 1 | 3 | 73 [14.77%, 22.33%] | 309 [72.89%, 81.09%] | 236 / 0 |
| 8 | 1 | 4 | 116 [24.77%, 33.63%] | 368 [88.92%, 94.28%] | 254 / 2 |

The selected IR schedules do not beat Chase on first-retry rescue at levels 1
and 2. Their margins are -0.25 and -6.75 percentage points. At level 4, the
single-codeword margin is +13.25 points, but the operating point is above the
requested calibration failure range. The eight-codeword frame fails on
400/402 initial transmissions at that same SNR. Combining rescues 20/400
on its first retry versus 8/400 for Chase, a +3.00-point margin, but that is
still only a 5% rescue rate. These results do not establish a session
throughput improvement or justify replacing Chase everywhere.

The plan-phase 120-trial probe, dated 2026-11-10 and recorded in the COP-168
plan, used different seeds and operating points. This larger paired diagnostic
replaces that probe for the shipped schedule. The level-3 AWGN 3 dB regression
guard uses 40 deterministic trials and passes in release mode with the shipped
RV3 first retry. It is release-only because the debug run exceeded 150 s.

## COP-187 experimental header repair

This is a development measurement of the COP-169 16-FSK candidate, not a
production floor or completion of COP-187. Modulation and duration remain 48 kHz,
50 baud, 16 tones from 600 to 1350 Hz, four header repetitions, 40 payload
bytes plus CRC32 under NR BG2, and exactly eight seconds of audio.

The old receiver hard-decided each header nibble before checking CRC16. The
repair combines normalized tone energies across repetitions and scores the 41
legal length/CRC combinations together. It requires independent version checks,
positive header evidence and a unique winner. Delivery still requires payload
FEC convergence, CRC32 over the unpadded application bytes and zero padding.
The CRC now binds the decoded length, so a header miscorrection cannot merely
append padding zeros. All sweep payloads use the full 40-byte capacity, where
this CRC coverage change leaves transmitted bytes identical. Failed header
acquisition remains in `sync_failed`; the measurement contract and channel mathematics are unchanged.

Paired Watterson Poor simulations use development seed base `0x00C0FFEE`, ten
trials at each of nine points from -12 to +4 dB. Payloads, channel draws and
unknown frame offsets are identical between receivers. Noise power references
clean active TX power in 3 kHz, not the faded output. The before receiver is
`019b161612b941a40412ce2164727b34d6915bd1`'s candidate, imported from this
repository. No external implementation was read or copied.

| SNR dB | Before frame errors | After frame errors | Before header failures | After header failures |
| --- | --- | --- | --- | --- |
| -12 | 10/10 | 10/10 | 5 | 4 |
| -10 | 7/10 | 6/10 | 7 | 6 |
| -8 | 4/10 | 3/10 | 4 | 3 |
| -6 | 3/10 | 1/10 | 3 | 1 |
| -4 through +4 | 0/50 | 0/50 | 0 | 0 |

Across 90 attempts, frame errors fall from 24 to 20. The highest interpolated
50% FER crossing moves from -8.67 to -9.33 dB. Both receivers still FAIL the
sync-share bar at -8 and -6 dB. Neither delivers a wrong payload. An AWGN
check of the repaired candidate has one payload failure at -12 dB in 90
attempts, giving a crossing upper bound of -12 dB on these development seeds only.

Raw counts, Wilson intervals, aggregate airtime, goodput and decoder metadata
are under [`results/cop-187/`](results/cop-187/). The audit in
`crates/coppa-bench/tests/floor_results.rs` recomputes the CSV fields and verdicts.
To reproduce the repaired Poor run:

```sh
cargo run --release -p coppa-bench --example floor_candidate -- --channel poor --trials 10 --seed 12648430 --out-dir results/cop-187/dev-header-poor
```

Use `--channel awgn` and `dev-header-awgn` for the AWGN development check.
The before CSV uses the same command on the imported COP-169 receiver.
These are batch simulations at zero CFO without SSB filtering. They do not
establish streaming acquisition, connect/ramp success, a production air version,
or the locked 200-trial final-seed acceptance bars.

The acceptance sweep interface itself now covers both production receive
paths: `floor_mode sweep --rx batch|streaming|both` sweeps the production
batch and `StreamingReceiver` paths over the same grid, seeds and channel
realizations, with one CSV row set and metadata verdict block per path. The
development CSVs above remain batch-only measurements of the experimental
candidate; no final-seed `--rx both` sweep has been recorded, and the
production floor waveform is now implemented as air-protocol version 2.
The `floor_mode` diagnostic still measures only the OFDM baseline.

## COP-189 continuous candidate acquisition slice

`floor_candidate --streaming` exercises continuous acquisition of the existing
experimental 16-FSK candidate. It ingests arbitrary chunks, decodes overlapping
windows on a one-second cadence, and keeps at most 480,000 audio samples. Each
window originally attempted at most 16 separated sync candidates at zero CFO.
The CFO search described below increases that bound. Decode work is independent
of caller chunk sizes and stream duration. Tone energies and candidate lists
remain bounded by the ten-second window; this implementation recomputes them
for each window rather than claiming an incremental matched-filter receiver.
Metadata records decode attempts and peak retained samples at chunk boundaries.
A missing streaming delivery counts conservatively as SyncFailed because this
adapter does not expose a validated failure-stage trace.

The release regression
`cargo test --release -p coppa-bench --lib continuous_acquisition_300_seconds_noise -- --ignored`
passed with zero deliveries for 300 seconds each of seeded white noise,
SSB-filtered one-second noise blocks and a 1 kHz tone plus noise. Each stream
attempted 293 decodes and retained at most 480,000 samples. This is synthetic
noise rejection evidence only. It does not select a production waveform or
resolve the floor FER, connection or fade-ramp acceptance requirements. No
final-seed production results are published for this slice.


### COP-189 CFO acquisition repair

The experimental batch and continuous receivers now share a matched-filter bank
covering 450..1500 Hz in 25 Hz steps. Thirteen views select the sixteen nominal
tones shifted by -150..+150 Hz. Each view may try sixteen separated candidates,
so a window attempts at most 208 candidate decodes. Only one sixteen-tone view
is allocated at a time. Audio retention and the one-second window cadence are
unchanged. Templates repeat after two symbols, including the half-bin offsets.
Transmit tones, framing, eight-second airtime and CRC checks are unchanged.

`cargo test --release -p coppa-bench --lib cfo_search_decodes_unknown_offset_in_batch_and_chunks`
requires exact payload delivery at -150, -75, +75 and +150 Hz, with an unknown
137-sample prefix, in batch and 317-sample chunks. It failed at -150 Hz before
the repair. This is a noiseless acquisition regression, not a fading FER curve.
Diagnostic metadata now records the grid and expanded work bound. Historical
COP-169 and COP-187 CSVs remain measurements of their original receivers.

The search holds CFO constant over the frame. It does not implement fading
tracking or prove reception between grid points. The BPSK comparison,
session integration and final sweep/connect/ramp evidence
remain unbuilt. COP-190 separately implements production v2 framing and
engine receive paths. No acceptance criterion is declared complete by this repair.

### COP-187 integer-tone CFO acquisition development check

This historical experimental batch 16-FSK receiver shared 22 matched-filter tones
across seven CFO hypotheses from -150 to +150 Hz in 50 Hz steps. It attempts
at most 16 timing/frequency candidates per capture. The transmitter is unchanged.
This is a development check, not production or final-seed acceptance.

These historical runs used revision `456661e`, before COP-189 expanded
the search grid. On that revision, reproduce each committed directory with
the following command, substituting `-150`, `0` and `150` and the
corresponding directory below:

```sh
cargo run --release -p coppa-bench --example floor_candidate -- --channel poor --trials 10 --seed 12648430 --cfo -150 --out-dir results/cop-187/dev-cfo-minus150-poor
```

All arms use development base `0x00C0FFEE`, the -12..+4 dB grid in 2 dB steps,
and ten attempts per point through the persistent seeded Poor channel. The
columns below aggregate 90 attempts. FER, Wilson intervals, crossings and
accounting are recomputed by `integer_tone_cfo_development_measurements_are_auditable`.

| CFO Hz | Directory under `results/cop-187/` | Frame errors | Sync failures | 50% crossing dB | Sync-share violations dB |
|---:|---|---:|---:|---:|---|
| -150 | `dev-cfo-minus150-poor` | 16 | 11 | -10.3333 | -10, -8 |
| 0 | `dev-cfo-zero-poor` | 20 | 16 | -9.3333 | -8, -6 |
| 150 | `dev-cfo-plus150-poor` | 16 | 10 | -10.0000 | -8 |

No arm delivered wrong payloads. All three commands exit 1 because acquisition
failures violate the unchanged sync-share bar above the crossing. Better
crossings do not select this candidate. The historical zero-CFO-only evidence
above remains intact. These measurements do not establish arbitrary fractional
CFO tolerance, continuous acquisition, production v2 framing or session recovery.
Unit tests separately cover clean SSB filtering with its FIR tail flushed and
one seeded -6 dB AWGN capture at 0 and +/-150 Hz. They do not establish a filtered
FER curve or a noisy SSB compatibility envelope.


## COP-190 production floor development measurements

Measured 2026-11-12, x86_64 Linux, release build. These are production
`FloorFrame::receive` and `FloorStream::push` measurements of air version 2,
not the experimental candidate. Reproduce through the bench test harness:

```sh
cargo test --release -p coppa-bench --test floor_results record_production_floor_development_sweeps -- --ignored --nocapture
cargo test -p coppa-bench --test floor_results production_floor_development_measurements_are_auditable
```

The nine-point −12..+4 dB grid uses ten 40-byte payloads per point, development
base `0x00C0FFEE`, and `attempt_seed(base, point, 10, trial)`. Each attempt
uses one persistent channel, direction 0, with noise referenced to clean active
power in 3 kHz. Batch and streaming see the same capture. It has 16,800..19,199
lead-in noise samples and 48,000 trailing noise samples to cover channel delay
and the next streaming decode interval. Streaming chunks are 4,800 samples.
CFO is zero and SSB filtering is off. Waveform airtime is 8 s, excluding noise
padding. Goodput counts delivered payload bits per waveform airtime.

| Channel | RX | Errors / attempts | 50% crossing dB | Sync-share violations dB |
|---|---|---:|---:|---|
| AWGN | batch | 2 / 90 | ≤ −12 | −12 |
| AWGN | streaming | 2 / 90 | ≤ −12 | −12 |
| Poor | batch | 21 / 90 | −9 | −8, −6 |
| Poor | streaming | 21 / 90 | −9 | −8, −6 |

All four arms deliver zero wrong payloads. All fail the unchanged sync-share
clause despite crossing below its threshold. The AWGN crossing is a bound,
not an extrapolated crossing. Streaming exposes no trusted failure-stage trace;
every absent delivery is conservatively counted as SyncFailed/NoDetection.
Batch separates acquisition, header and payload failures without claiming
mistiming detection. The CSV audit recomputes counts, FER, Wilson 95% intervals,
airtime, goodput, crossings and verdicts, including failed trials.

Raw CSVs and metadata are in `results/cop-190/dev-awgn/` and
`results/cop-190/dev-poor/`. These ten-trial development runs do not establish
the 200-trial final-seed bar, negotiated sessions, connection success or fade
recovery. The floor-aware Link/daemon controller and connection trials are
still missing. COP-191 owns the final-seed sweeps, Poor ramp and diagnostic
drivers.
