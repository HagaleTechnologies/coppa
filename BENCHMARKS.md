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

## Log index

<<<<<<< HEAD
Every section title of the pre-COP-146 file, in its original order. Grouped logs retain each section unchanged.
=======
## COP-169: floor-mode measurement contract and the `ofdm:1` baseline

Measured 2026-11-11 on x86_64 Linux, release build, with the new
`floor_mode` diagnostic. There is no production floor waveform yet; these rows are the
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

## COP-129: golden vectors for levels 3, 4, 7 and 9, multi-codeword frames and retransmission
>>>>>>> f28564c475ccc00e6c4ea047a3a598ec94c23329

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
