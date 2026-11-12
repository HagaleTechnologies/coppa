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

<<<<<<< HEAD
The base seed is `12648430` (`0x00C0FFEE`), with 200 independent frames at
each of eleven points. Raw counts and failure categories are in
`results/cop-67-hf-narrow/awgn.csv`. FER is `frame_errors / trials`. The SNR
uses the same `coppa_channel::awgn_ref_seeded` convention as the existing
level sweeps: the clean transmitted frame's mean power (including preamble,
probe, and header) is compared with noise in a 3 kHz reference bandwidth;
white noise is added across the 24 kHz Nyquist band. A claim of FER ≤ 10%
also requires the 95% Wilson **upper** bound to be ≤ 0.10.

| SNR (dB) | Frame errors / 200 | FER | 95% Wilson interval |
|---:|---:|---:|---:|
| −6 | 200 | 1.000 | 0.981–1.000 |
| −5 | 200 | 1.000 | 0.981–1.000 |
| −4 | 200 | 1.000 | 0.981–1.000 |
| −3 | 200 | 1.000 | 0.981–1.000 |
| −2 | 200 | 1.000 | 0.981–1.000 |
| −1 | 200 | 1.000 | 0.981–1.000 |
| 0 | 200 | 1.000 | 0.981–1.000 |
| +1 | 198 | 0.990 | 0.964–0.997 |
| +2 | 182 | 0.910 | 0.862–0.942 |
| +3 | 112 | 0.560 | 0.491–0.627 |
| +4 | 33 | 0.165 | 0.120–0.223 |

**Result: the weak-signal target failed.** Every frame at every measured
subzero point was lost; even +4 dB has FER 16.5% (Wilson upper bound 22.3%).
The benchmark's `SyncFailed` category accounts for these losses, but that
category also includes protected-header decode failures after candidate
detection, so it does not by itself locate the limit. A separate probe in
the COP-67 research ran 100 trials per point through the same batch receive
filter and found no sync candidate at subzero SNR; when a correctly timed
candidate existed, it observed no header or payload failures. The
profile-independent sync detector's plateau and confirm thresholds are a
plausible mechanism, but lower thresholds and their false-alarm trade-off
were not measured. No operator profile was enabled. This is an AWGN batch
simulation, with no hardware or on-air result.

## COP-126: decimated sync pre-filter on the streaming receive path

`StreamingReceiver` (the daemon, `coppa rx` and the FFI) now builds its
sync detector with `SyncDetector::new_prefiltered`: a 121-tap 250-2850 Hz
band-pass decimated by 4 ahead of the Schmidl-Cox plateau search, with the
confirm and timing steps still at 48 kHz on raw samples
(`docs/adr/011-cop126-decimated-sync-prefilter.md`). This section measures
the sync threshold of both front ends on the input the streaming receiver
sees: full-band white noise with no RX filter.

Reproduce:

```sh
cargo run --release -p coppa-bench --example streaming_sync_threshold
```

The base seed is `12648430` (`0x00C0FFEE`), with 60 trials per point; trial
`i` uses noise seed `0x00C0FFEE + i`. The SNR uses the same
`coppa_channel::awgn_ref_seeded` convention as the other level sweeps: the
whole clean frame's mean power is compared with noise in a 3 kHz reference
bandwidth, and white noise is added across the 24 kHz Nyquist band. Each
trial stream is 4 symbols of silence, the first 6 symbols of an
`hf_standard` frame (preamble, probe and header) and 2 symbols of silence.
A detection is correct when the first candidate lies within 60 samples of
the candidate the same front end finds on the noise-free stream, which
separates sync from FEC. "raw" is `SyncDetector::new`, the front end the
streaming receiver used before COP-126.

Output of the committed program (release build, this container):


### Level 1

| SNR (dB) | 1.5 | 2 | 2.5 | 3 | 3.5 | 4 | 4.5 | 5 | 5.5 | 6 | 6.5 | 7 | 11.5 | 11.75 | 12 | 12.25 | 12.5 | 12.75 | 13 | 13.25 | 13.5 | 13.75 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| raw | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 25 | 45 | 58 | 60 | 60 | 60 | 60 | 60 |
| prefiltered | 10 | 19 | 36 | 47 | 57 | 59 | 60 | 60 | 59 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 |

| front end | 50% point (dB) | 90% point (dB) | gain at 50% (dB) | gain at 90% (dB) |
|---|---|---|---|---|
| raw | 12.31 | 12.67 | 0.00 | 0.00 |
| prefiltered | 2.32 | 3.35 | 9.99 | 9.32 |

### Level 2

| SNR (dB) | 1.5 | 2 | 2.5 | 3 | 3.5 | 4 | 4.5 | 5 | 5.5 | 6 | 6.5 | 7 | 11.5 | 11.75 | 12 | 12.25 | 12.5 | 12.75 | 13 | 13.25 | 13.5 | 13.75 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| raw | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 4 | 19 | 43 | 56 | 60 | 60 | 60 | 60 | 60 |
| prefiltered | 9 | 18 | 36 | 46 | 56 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 |

| front end | 50% point (dB) | 90% point (dB) | gain at 50% (dB) | gain at 90% (dB) |
|---|---|---|---|---|
| raw | 12.36 | 12.71 | 0.00 | 0.00 |
| prefiltered | 2.33 | 3.40 | 10.03 | 9.31 |

### Cost

raw: SyncDetector::push realtime factor 0.00788x (10 s of noise, 512-sample chunks, best of 5)
prefiltered: SyncDetector::push realtime factor 0.00557x (10 s of noise, 512-sample chunks, best of 5)

The pre-filter moves the 50% sync point down by about 10 dB at both levels
(9.3 dB at the 90% point), more than the 6 dB the ticket asked for. The
pre-filtered detector also costs about 30% less per sample than the raw
one, because the plateau search runs on a quarter of the samples. Neither
front end changes what the batch path does: `CoppaTransceiver` still
filters with its 601-tap `rx_bpf` and calls `SyncDetector::detect_all`,
which uses `new`.

Prototype measurements from the COP-126 plan phase, not re-run on the
committed code: on `hf_standard` with full-band AWGN, 20 frames per cell
fed in 4800-sample chunks, the streaming receiver decoded 0/20 at 3, 6 and
10 dB for levels 1 to 7 before the change, and matched or beat the batch
API in every cell after it (for example 20/20 at 6 dB for levels 1 to 6).
The golden-vector candidates were sample-identical between the two front
ends on every HF vector, including both Watterson-Poor vectors (`L1_poor25`
at 367, `L2_poor25` at 274). 300 s each of white noise, SSB-band noise and a
1 kHz tone with noise gave no candidate from either front end. This is an
AWGN simulation, with no Watterson sweep of the streaming path and no
hardware or on-air result.

### `--cfo ±150` sweep (AWGN + ±150 Hz carrier offset, level 2)

COP-128, 2026-11-04. `hf_standard`, seed `0x00C0FFEE`, 20 trials per cell,
3 kHz reference-band SNR. The hypothesis search is per-profile and independent
of speed level; this table measures level 2 only. Reproduce each sign with:

```sh
cargo run --release -p coppa-bench -- --level 2 --profile standard --cfo 150 --trials 20 --snr-min 0 --snr-max 18 --snr-step 3 --out-dir /tmp/cop128-plus
cargo run --release -p coppa-bench -- --level 2 --profile standard --cfo -150 --trials 20 --snr-min 0 --snr-max 18 --snr-step 3 --out-dir /tmp/cop128-minus
```

| SNR (dB) | +150 Hz errors/trials | −150 Hz errors/trials | +150 Hz goodput (bps) | −150 Hz goodput (bps) |
|---|---|---|---|---|
| 0.0 | 20/20 | 20/20 | 0.00 | 0.00 |
| 3.0 | 5/20 | 5/20 | 514.29 | 514.29 |
| 6.0 | 1/20 | 1/20 | 651.43 | 651.43 |
| 9.0 | 0/20 | 1/20 | 685.71 | 651.43 |
| 12.0 | 0/20 | 0/20 | 685.71 | 685.71 |
| 15.0 | 0/20 | 0/20 | 685.71 | 685.71 |
| 18.0 | 0/20 | 0/20 | 685.71 | 685.71 |

All observed errors were sync failures. Wilson 95% FER intervals are
[83.89%, 100%] for 20/20 errors, [11.19%, 46.87%] for 5/20,
[0.89%, 23.61%] for 1/20 and [0%, 16.11%] for 0/20. Twenty trials
cannot establish either a 10% or a 1% FER ceiling at 95% confidence.
At 12 dB both signs decoded all 20 frames. The CI guard
`waterfall_cfo150_level_2` allows at most one error per 20-trial sign.

## COP-119: RS(12,6) session fade protection

Measured on 2026-11-06 with workspace version 0.8.0, on the COP-119 branch.
Command: `cargo run --release -p coppa-bench --example session`.
The original window-eight bare-frame baseline is unchanged. The new
`stop-and-wait` and `rs12-6` arms include MAC and transport headers, window
one, a 1200-sample RX tail, and independent channel seeds for every physical
transmission. They use the production outer packetizer, reassembler and ARQ.
Each fading summary row averages five trials, including dropped trials. These are simulated
ten-minute sessions with ideal ACKs unless explicitly labeled ACK-loss.
The channel is reset per packet, not a continuous on-air fading realization.
A retry that `get_retransmits` hands out is still permitted, so the framed
arms transmit it and give it its ACK opportunity before a drop is recorded,
matching the daemon's send-then-mark order (the 2026-11-04 run predated this
and recorded drops one retransmission early); dropped trials print the number
of sends of the segment that exhausted the budget.

The ACK-loss arm sends ACK frames through the same channel. Protected blocks
use two copies and the sender's conservative extra hold if only the second
copy decodes. See SPEC §22.4. This arm is reported separately from the
Moderate 5/5 acceptance policy. No seeds or retry constants were tuned.

Clean cable, using the two real daemon event loops and a deterministic
2000-byte host write, measured default `accept` at **2706.4 B/min** in
44.34 seconds and `request` at **1724.1 B/min** in 69.60 seconds. Protected
mode was 36.3% slower on this payload. The cable harness includes handshake,
framing, PTT and host delivery; the bench's clean row isolates steady-state
session transfers. The cable run took 115.20 seconds wall time and checked
that keyed intervals from the two stations did not overlap.

Reproduce the cable report with
`cargo test -p coppa-daemon --lib --features cpal-backend,websocket rs_cable_clean_throughput_report -- --ignored --nocapture`.

Measurement wall time: 511.2 seconds for the session executable, excluding its release build.

The protected ideal-ACK arm completed Good, Moderate and Poor at 5/5.
With ACK loss it completed Good, Moderate and Poor at 5/5.
This does not prove RF reliability or survival through a continuous channel.

Full session output:

```text
=== Session-robustness bench: 5 x 10-min ARQ sessions/preset ===
Level 2 (BPSK 1/2), window 8, SNR ramp 20->0->20 dB over the session.
Drop = ArqTx::is_failed (RTO cap reached), not a single retransmit.

  [good] trial 0: completed (602.7s, 15912 bytes, 1584.1 bytes/min)
  [good] trial 1: DROPPED (491.9s, 9126 bytes, 1113.1 bytes/min)
  [good] trial 2: completed (600.0s, 15795 bytes, 1579.4 bytes/min)
  [good] trial 3: DROPPED (449.5s, 8892 bytes, 1186.9 bytes/min)
  [good] trial 4: DROPPED (449.9s, 8307 bytes, 1107.9 bytes/min)
-> good: 2/5 sessions completed without a drop, avg net goodput 1314.3 bytes/min

  [moderate] trial 0: DROPPED (399.3s, 6903 bytes, 1037.2 bytes/min)
  [moderate] trial 1: DROPPED (403.8s, 7137 bytes, 1060.6 bytes/min)
  [moderate] trial 2: DROPPED (371.5s, 6669 bytes, 1077.0 bytes/min)
  [moderate] trial 3: DROPPED (408.8s, 7137 bytes, 1047.6 bytes/min)
  [moderate] trial 4: DROPPED (368.5s, 6552 bytes, 1066.8 bytes/min)
-> moderate: 0/5 sessions completed without a drop, avg net goodput 1057.8 bytes/min

  [poor] trial 0: DROPPED (293.3s, 1755 bytes, 359.0 bytes/min)
  [poor] trial 1: DROPPED (266.4s, 1638 bytes, 368.9 bytes/min)
  [poor] trial 2: DROPPED (246.4s, 1521 bytes, 370.4 bytes/min)
  [poor] trial 3: DROPPED (250.5s, 1404 bytes, 336.3 bytes/min)
  [poor] trial 4: DROPPED (279.8s, 1287 bytes, 276.0 bytes/min)
-> poor: 0/5 sessions completed without a drop, avg net goodput 342.1 bytes/min

=== Acceptance policy: Good deterministic regression floor ===
MET: Good completed 2/5 sessions without a drop (required at least 2/5).
Moderate and Poor are diagnostic-only; their results remain reported above.

=== COP-119 daemon-matched window-1 session arms ===
-> stop-and-wait ideal-ACK clean: 1901.0 bytes/min
  stop-and-wait ideal-ACK [good] trial 0: completed (601.9s, 12384 bytes, 1234.4 bytes/min)
  stop-and-wait ideal-ACK [good] trial 1: completed (601.7s, 7968 bytes, 794.5 bytes/min)
  stop-and-wait ideal-ACK [good] trial 2: DROPPED after 6 sends (402.4s, 6528 bytes, 973.4 bytes/min)
  stop-and-wait ideal-ACK [good] trial 3: DROPPED after 6 sends (325.6s, 6624 bytes, 1220.7 bytes/min)
  stop-and-wait ideal-ACK [good] trial 4: completed (600.7s, 13152 bytes, 1313.6 bytes/min)
-> stop-and-wait ideal-ACK good: 3/5 completed, 1107.3 bytes/min
  stop-and-wait ideal-ACK [moderate] trial 0: completed (602.5s, 10944 bytes, 1089.8 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 1: completed (601.6s, 12864 bytes, 1283.0 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 2: completed (601.5s, 11232 bytes, 1120.4 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 3: DROPPED after 6 sends (353.2s, 6336 bytes, 1076.3 bytes/min)
  stop-and-wait ideal-ACK [moderate] trial 4: completed (600.5s, 11328 bytes, 1131.8 bytes/min)
-> stop-and-wait ideal-ACK moderate: 4/5 completed, 1140.3 bytes/min
  stop-and-wait ideal-ACK [poor] trial 0: completed (600.0s, 1920 bytes, 192.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 1: completed (601.1s, 5280 bytes, 527.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 2: DROPPED after 6 sends (345.5s, 576 bytes, 100.0 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 3: completed (600.8s, 4896 bytes, 488.9 bytes/min)
  stop-and-wait ideal-ACK [poor] trial 4: DROPPED after 6 sends (450.7s, 2976 bytes, 396.2 bytes/min)
-> stop-and-wait ideal-ACK poor: 3/5 completed, 340.8 bytes/min
-> stop-and-wait ACK-loss clean: 1901.0 bytes/min
  stop-and-wait ACK-loss [good] trial 0: DROPPED after 6 sends (341.5s, 5856 bytes, 1029.0 bytes/min)
  stop-and-wait ACK-loss [good] trial 1: completed (601.9s, 9024 bytes, 899.5 bytes/min)
  stop-and-wait ACK-loss [good] trial 2: completed (600.3s, 9888 bytes, 988.4 bytes/min)
  stop-and-wait ACK-loss [good] trial 3: DROPPED after 6 sends (341.6s, 5952 bytes, 1045.5 bytes/min)
  stop-and-wait ACK-loss [good] trial 4: DROPPED after 6 sends (325.8s, 6048 bytes, 1113.8 bytes/min)
-> stop-and-wait ACK-loss good: 2/5 completed, 1015.3 bytes/min
  stop-and-wait ACK-loss [moderate] trial 0: DROPPED after 6 sends (319.8s, 4896 bytes, 918.7 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 1: completed (600.0s, 8832 bytes, 883.2 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 2: DROPPED after 6 sends (324.8s, 4800 bytes, 886.6 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 3: DROPPED after 6 sends (285.8s, 5088 bytes, 1068.2 bytes/min)
  stop-and-wait ACK-loss [moderate] trial 4: completed (600.1s, 9216 bytes, 921.5 bytes/min)
-> stop-and-wait ACK-loss moderate: 2/5 completed, 935.6 bytes/min
  stop-and-wait ACK-loss [poor] trial 0: completed (600.3s, 672 bytes, 67.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 1: DROPPED after 6 sends (451.9s, 672 bytes, 89.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 2: completed (600.3s, 672 bytes, 67.2 bytes/min)
  stop-and-wait ACK-loss [poor] trial 3: completed (600.2s, 2016 bytes, 201.5 bytes/min)
  stop-and-wait ACK-loss [poor] trial 4: DROPPED after 6 sends (175.5s, 1632 bytes, 558.1 bytes/min)
-> stop-and-wait ACK-loss poor: 3/5 completed, 196.6 bytes/min
-> rs12-6 ideal-ACK clean: 1814.5 bytes/min
  rs12-6 ideal-ACK [good] trial 0: completed (607.9s, 14675 bytes, 1448.5 bytes/min)
  rs12-6 ideal-ACK [good] trial 1: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [good] trial 2: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [good] trial 3: completed (610.9s, 14675 bytes, 1441.3 bytes/min)
  rs12-6 ideal-ACK [good] trial 4: completed (610.9s, 14675 bytes, 1441.3 bytes/min)
-> rs12-6 ideal-ACK good: 5/5 completed, 1518.5 bytes/min
  rs12-6 ideal-ACK [moderate] trial 0: completed (612.4s, 14088 bytes, 1380.4 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 1: completed (611.9s, 14088 bytes, 1381.5 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 2: completed (604.7s, 16436 bytes, 1630.7 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 3: completed (607.9s, 14675 bytes, 1448.5 bytes/min)
  rs12-6 ideal-ACK [moderate] trial 4: completed (609.4s, 14675 bytes, 1444.9 bytes/min)
-> rs12-6 ideal-ACK moderate: 5/5 completed, 1457.2 bytes/min
  rs12-6 ideal-ACK [poor] trial 0: completed (610.2s, 9979 bytes, 981.1 bytes/min)
  rs12-6 ideal-ACK [poor] trial 1: completed (600.8s, 10566 bytes, 1055.2 bytes/min)
  rs12-6 ideal-ACK [poor] trial 2: completed (600.8s, 10566 bytes, 1055.2 bytes/min)
  rs12-6 ideal-ACK [poor] trial 3: completed (607.1s, 11153 bytes, 1102.3 bytes/min)
  rs12-6 ideal-ACK [poor] trial 4: completed (619.2s, 10566 bytes, 1023.9 bytes/min)
-> rs12-6 ideal-ACK poor: 5/5 completed, 1043.6 bytes/min
-> rs12-6 ACK-loss clean: 1814.5 bytes/min
  rs12-6 ACK-loss [good] trial 0: completed (613.8s, 14675 bytes, 1434.5 bytes/min)
  rs12-6 ACK-loss [good] trial 1: completed (600.9s, 12327 bytes, 1230.8 bytes/min)
  rs12-6 ACK-loss [good] trial 2: completed (613.3s, 14675 bytes, 1435.6 bytes/min)
  rs12-6 ACK-loss [good] trial 3: completed (612.3s, 14675 bytes, 1438.1 bytes/min)
  rs12-6 ACK-loss [good] trial 4: completed (611.8s, 14675 bytes, 1439.2 bytes/min)
-> rs12-6 ACK-loss good: 5/5 completed, 1395.7 bytes/min
  rs12-6 ACK-loss [moderate] trial 0: completed (602.0s, 14088 bytes, 1404.1 bytes/min)
  rs12-6 ACK-loss [moderate] trial 1: completed (606.1s, 13501 bytes, 1336.6 bytes/min)
  rs12-6 ACK-loss [moderate] trial 2: completed (615.0s, 14675 bytes, 1431.7 bytes/min)
  rs12-6 ACK-loss [moderate] trial 3: completed (605.4s, 13501 bytes, 1338.0 bytes/min)
  rs12-6 ACK-loss [moderate] trial 4: completed (609.1s, 14088 bytes, 1387.8 bytes/min)
-> rs12-6 ACK-loss moderate: 5/5 completed, 1379.6 bytes/min
  rs12-6 ACK-loss [poor] trial 0: completed (614.4s, 5283 bytes, 515.9 bytes/min)
  rs12-6 ACK-loss [poor] trial 1: completed (602.2s, 7631 bytes, 760.3 bytes/min)
  rs12-6 ACK-loss [poor] trial 2: completed (618.0s, 7044 bytes, 683.9 bytes/min)
  rs12-6 ACK-loss [poor] trial 3: completed (613.8s, 8805 bytes, 860.7 bytes/min)
  rs12-6 ACK-loss [poor] trial 4: completed (607.5s, 8805 bytes, 869.6 bytes/min)
-> rs12-6 ACK-loss poor: 5/5 completed, 738.1 bytes/min
MET: rs12-6 Moderate completed 5/5 sessions without a drop (required 5/5).
```

## COP-133: Same-frame residual experiment

2026-11-04. Partial implementation; the rate-loop bar remains unmet. The candidate
was not adopted. Production recommendations, controller settings, waveforms,
LDPC noise vectors, SNR telemetry and comparator definitions are unchanged.

The checkout was reconciled from `02650835d98c6519c7fdb4a12693af6523717874`
to main `b8af910712b134f1dec2e0e645cdbe8a49005ba3` before the retained runs.
The only production refactor extracts the existing payload-to-constellation chain
into a private helper also called by transmit. A frozen copy of the original
chain verifies identity at all nine levels, RVs 0 through 3, codeword counts
1, 2 and 8, and empty, one-byte and full payloads.

### Applicability experiment

Command:

```sh
cargo test -p coppa-protocol --release --lib residual_recommendation_grid -- --ignored --nocapture
```

Training base `0xCA11B`, held-out base `0x5A1AD`; eight seeds per base, all nine
measured levels, AWGN and Watterson Good/Moderate/Poor, SNR grid 6/12/18/24/30 dB,
`hf_robust`. Each trial evicts HARQ state. Reconstruction uses the CRC-verified
payload and current RV. Turbo rescues fall back because the experiment does not
retain their re-equalized observations. Residual pooling follows the alternating
physical-carrier map, excludes unused padding and rejects missing coded positions.
Only capacity conversion floors residual variance at 1e-6. No pilot PAPR-bias
correction or new calibration constants are applied.

All 2,880 receive attempts completed. No successful probe provided complete
observations, so every recommendation fell back to the pilot result. For example,
level 1 reconstructs 1,944 positions but frame-sized HF input yields only 1,908
equalized positions after RX bandpass filtering. A deterministic normal test
reproduces this and shows that test-only trailing receive context restores the
complete geometry. The production capture and decoder are unchanged.

The following paired records are grouped by base, preset and measured level.
`ok/fail` counts CRC-successful probes and failed receive attempts out of 40.
`bps` is mean delivered payload bits divided by the chosen level's transmitted
sample duration, conditional on successful probe decoding. Candidate and pilot
values are identical. `regret` is the mean difference from the same-realization
nine-level oracle in those units; `miss` counts choices with lower goodput than
that oracle. These conditional observations are not the closed-loop acceptance
metric. Complete residual observations and changed recommendations are both zero
in every row. No claim of improved recommendation accuracy or next-frame feedback
value follows from these data.

| Base | Preset | Measured level | ok/fail | Pilot = candidate bps | Regret bps | miss |
|---|---|---:|---:|---:|---:|---:|
| ca11b | AWGN | 1 | 40/0 | 3381.406 | 156.916 | 10 |
| ca11b | AWGN | 2 | 40/0 | 3381.406 | 156.916 | 10 |
| ca11b | AWGN | 3 | 40/0 | 3224.490 | 313.832 | 16 |
| ca11b | AWGN | 4 | 40/0 | 3224.490 | 313.832 | 16 |
| ca11b | AWGN | 5 | 40/0 | 3010.431 | 527.891 | 27 |
| ca11b | AWGN | 6 | 40/0 | 3086.621 | 451.701 | 23 |
| ca11b | AWGN | 7 | 40/0 | 3010.431 | 527.891 | 27 |
| ca11b | AWGN | 8 | 36/4 | 3036.735 | 607.710 | 28 |
| ca11b | AWGN | 9 | 30/10 | 3127.438 | 643.991 | 22 |
| ca11b | Good | 1 | 40/0 | 609.137 | 765.757 | 22 |
| ca11b | Good | 2 | 40/0 | 569.249 | 805.645 | 23 |
| ca11b | Good | 3 | 21/19 | 469.732 | 974.442 | 20 |
| ca11b | Good | 4 | 15/25 | 657.625 | 944.723 | 14 |
| ca11b | Good | 5 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Good | 6 | 10/30 | 848.980 | 1203.084 | 9 |
| ca11b | Good | 7 | 4/36 | 1697.959 | 885.261 | 4 |
| ca11b | Good | 8 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Good | 9 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Moderate | 1 | 40/0 | 481.035 | 558.745 | 24 |
| ca11b | Moderate | 2 | 35/5 | 493.084 | 655.267 | 25 |
| ca11b | Moderate | 3 | 14/26 | 752.301 | 844.114 | 12 |
| ca11b | Moderate | 4 | 6/34 | 699.160 | 896.359 | 6 |
| ca11b | Moderate | 5 | 5/35 | 1743.586 | 182.509 | 4 |
| ca11b | Moderate | 6 | 10/30 | 848.980 | 963.048 | 10 |
| ca11b | Moderate | 7 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Moderate | 8 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Moderate | 9 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Poor | 1 | 33/7 | 178.627 | 587.163 | 23 |
| ca11b | Poor | 2 | 23/17 | 364.467 | 612.630 | 13 |
| ca11b | Poor | 3 | 10/30 | 292.272 | 1195.146 | 10 |
| ca11b | Poor | 4 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Poor | 5 | 5/35 | 1697.959 | 228.136 | 5 |
| ca11b | Poor | 6 | 5/35 | 1697.959 | 228.136 | 5 |
| ca11b | Poor | 7 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Poor | 8 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Poor | 9 | 0/40 | 0.000 | 0.000 | 0 |
| 5a1ad | AWGN | 1 | 40/0 | 3381.406 | 207.937 | 14 |
| 5a1ad | AWGN | 2 | 40/0 | 3400.454 | 188.889 | 13 |
| 5a1ad | AWGN | 3 | 40/0 | 3262.585 | 326.757 | 18 |
| 5a1ad | AWGN | 4 | 40/0 | 3243.537 | 345.805 | 19 |
| 5a1ad | AWGN | 5 | 40/0 | 3010.431 | 578.912 | 30 |
| 5a1ad | AWGN | 6 | 40/0 | 3083.537 | 505.805 | 26 |
| 5a1ad | AWGN | 7 | 40/0 | 3067.574 | 521.769 | 27 |
| 5a1ad | AWGN | 8 | 39/1 | 3001.849 | 613.291 | 31 |
| 5a1ad | AWGN | 9 | 31/9 | 3109.882 | 661.546 | 23 |
| 5a1ad | Good | 1 | 40/0 | 806.831 | 1306.054 | 39 |
| 5a1ad | Good | 2 | 35/5 | 933.126 | 1279.553 | 35 |
| 5a1ad | Good | 3 | 30/10 | 924.576 | 1226.347 | 29 |
| 5a1ad | Good | 4 | 20/20 | 1247.859 | 686.915 | 20 |
| 5a1ad | Good | 5 | 20/20 | 979.505 | 1176.584 | 20 |
| 5a1ad | Good | 6 | 34/6 | 1461.679 | 732.590 | 29 |
| 5a1ad | Good | 7 | 17/23 | 1697.959 | 885.261 | 17 |
| 5a1ad | Good | 8 | 0/40 | 0.000 | 0.000 | 0 |
| 5a1ad | Good | 9 | 0/40 | 0.000 | 0.000 | 0 |
| 5a1ad | Moderate | 1 | 40/0 | 220.435 | 2025.522 | 40 |
| 5a1ad | Moderate | 2 | 35/5 | 435.589 | 1888.653 | 34 |
| 5a1ad | Moderate | 3 | 21/19 | 1178.625 | 976.076 | 19 |
| 5a1ad | Moderate | 4 | 16/24 | 1364.275 | 711.843 | 14 |
| 5a1ad | Moderate | 5 | 20/20 | 1199.978 | 1074.228 | 19 |
| 5a1ad | Moderate | 6 | 34/6 | 1685.266 | 656.595 | 27 |
| 5a1ad | Moderate | 7 | 17/23 | 1697.959 | 885.261 | 17 |
| 5a1ad | Moderate | 8 | 4/36 | 0.000 | 3009.524 | 4 |
| 5a1ad | Moderate | 9 | 0/40 | 0.000 | 0.000 | 0 |
| 5a1ad | Poor | 1 | 35/5 | 756.286 | 1617.457 | 31 |
| 5a1ad | Poor | 2 | 35/5 | 682.529 | 1691.214 | 31 |
| 5a1ad | Poor | 3 | 33/7 | 913.812 | 1302.666 | 28 |
| 5a1ad | Poor | 4 | 19/21 | 864.332 | 1118.503 | 14 |
| 5a1ad | Poor | 5 | 20/20 | 1426.808 | 748.829 | 14 |
| 5a1ad | Poor | 6 | 29/11 | 1924.918 | 541.441 | 22 |
| 5a1ad | Poor | 7 | 19/21 | 2070.701 | 692.016 | 15 |
| 5a1ad | Poor | 8 | 8/32 | 1697.959 | 1311.565 | 8 |
| 5a1ad | Poor | 9 | 0/40 | 0.000 | 0.000 | 0 |

This rejects applicability under the planned complete-observation requirement,
not residual estimation in general. No positive deterministic improvement fixture
was found, so the residual code stays in the private test experiment. It adds no
production receive encode or mapping work, and no receive CPU overhead comparison
is claimed. Reconstructing missing observations by changing the decoder, truncating
the expected stream or changing the benchmark capture would be a different experiment.

### Ground-truth diagnostic correction

```sh
cargo run -p coppa-bench --release --example capacity_ground_truth_diagnosis
```

All three 300-trial presets completed at 24 dB, measured level 1, robust profile.
The historical zero-filled statistics remain in the diagnostic output. Additional
statistics exclude failed demodulations without changing the successful pairs:

| Preset | Demodulated/failed | Capacity correlation | Selectivity correlation |
|---|---:|---:|---:|
| Good | 300/0 | 0.050 | 0.131 |
| Poor | 284/16 | -0.028 | 0.051 |
| Moderate | 299/1 | -0.230 | -0.140 |

The full-frame/second-half windows overlap. Model truth averages all active
carriers while measured noise follows alternating data carriers. The diagnostic
uses the raw modem without the transceiver RX bandpass, and AWGN references each
faded frame's realized power. These method differences and correlations establish
no physical ceiling. Historical raw numbers in LIMITATIONS.md are preserved.

### Full closed-loop controls

Both commands completed with five seeds `[0,1,2,3,4]`, 300 frames per seed,
and no smoke overrides:

```sh
cargo run -p coppa-bench --release --example closed_loop_arq
cargo run -p coppa-bench --release --example closed_loop_arq -- standard
```

These are pre-integration controls. Since the candidate was not adopted, the
final production path has identical recommendations and transmitted symbols.
No separate after-variant run or paired fading-segment goodput is claimed.
All transmitted frames, including failures, remain charged to airtime. The
comparator still contains all 18 level/CP cells. Robust uses a synthetic short-CP
twin, while standard uses the production standard/standard-short-CP profile pair.
The x1 column charges one five-frame handshake per CP switch; x0 is supplemental.
Aggregate ratios are the program's mean adaptive goodput divided by mean
comparator goodput, never means of per-seed ratios. Values below are the program's
rounded output; no unrounded passing verdict is inferred from them.

#### Robust control

| Seed | B delivered | B payload bits | Data air s | Switch air s | B x0 bps | B x1 bps | Joint best fixed bps | Joint oracle bps |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 | 257/300 | 326456 | 173.45 | 7.61 | 1882.1 | 1803.0 | 2250.5 | 3260.0 |
| 1 | 257/300 | 341888 | 160.25 | 7.61 | 2133.5 | 2036.8 | 2230.8 | 3218.3 |
| 2 | 259/300 | 330896 | 189.55 | 7.61 | 1745.7 | 1678.3 | 2338.9 | 3314.6 |
| 3 | 260/300 | 328584 | 171.01 | 7.61 | 1921.4 | 1839.5 | 2309.5 | 3269.8 |
| 4 | 258/300 | 329680 | 178.03 | 7.61 | 1851.9 | 1775.9 | 2221.0 | 3248.4 |

Legacy long-CP arm A control, delivered application bits per 300 frame slots.
These ratios are not comparable to the airtime ratios above.

| Seed | Adaptive bits | Best fixed bits | Oracle bits | a/bf | a/oracle |
|---|---:|---:|---:|---:|---:|
| 0 | 330144 | 358848 | 429256 | 0.920010701 | 0.769107479 |
| 1 | 325680 | 350304 | 427056 | 0.929706769 | 0.762616612 |
| 2 | 318512 | 367392 | 429416 | 0.866954098 | 0.741732958 |
| 3 | 325208 | 350304 | 427552 | 0.928359368 | 0.760627947 |
| 4 | 323888 | 364544 | 430880 | 0.888474368 | 0.751689566 |

Aggregate decision output, x0 then x1:

```text
arm B / best-fixed(joint) = 0.840 (x0) / 0.805 (x1)  (bar: > 1.0)
arm B / oracle(joint)     = 0.585 (x0) / 0.560 (x1)  (bar: >= 0.8)
```

Existing delay-spread segment output, retained as descriptive context only.
It does not measure fading-segment delivered-goodput non-regression.

```text
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up        97       0       0       0       0       0       0       0         3         0
s0 awgn ramp down      88       0       0       0       0       0       0       0        12         0
s0 watterson good      23      10       0       0       0       0       0       0        17         0
s0 watterson poor      23       0       0       0       0      16       0       0        11         0
s1 awgn ramp up        95       0       0       0       0       0       0       0         5         0
s1 awgn ramp down      91       0       0       0       0       0       0       0         9         0
s1 watterson good      27       8       0       0       0       0       0       0        15         0
s1 watterson poor      20       1       0       0       1      14       0       0        14         0
s2 awgn ramp up        97       0       0       0       0       0       0       0         3         0
s2 awgn ramp down      92       0       0       0       0       0       0       0         8         0
s2 watterson good      23       9       0       0       0       0       0       0        18         0
s2 watterson poor      22       0       0       0       0      16       0       0        12         0
s3 awgn ramp up        95       0       0       0       0       0       0       0         5         0
s3 awgn ramp down      90       0       0       0       0       0       0       0        10         0
s3 watterson good      30       5       0       0       0       0       0       0        15         0
s3 watterson poor      23       0       0       0       0      17       0       0        10         0
s4 awgn ramp up        93       0       0       0       0       0       0       0         7         0
s4 awgn ramp down      90       0       0       0       0       0       0       0        10         0
s4 watterson good      29       8       0       0       0       0       0       0        13         0
s4 watterson poor      21       0       0       0       0      17       0       0        12         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)

```

This run does not exactly reproduce the historical 0.914/0.762 record. Its
current robust seed-0 legacy ratios remain below both thresholds.
The robust seed-0 legacy bar fails exactly: `330144 <= 358848` and
`5 * 330144 < 4 * 429256`. Neither >1.0 versus best fixed nor >=0.8 versus oracle holds.

#### Standard control

| Seed | B delivered | B payload bits | Data air s | Switch air s | B x0 bps | B x1 bps | Joint best fixed bps | Joint oracle bps |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 | 273/300 | 351128 | 152.99 | 6.49 | 2295.1 | 2201.8 | 2491.4 | 3631.2 |
| 1 | 276/300 | 359616 | 156.81 | 6.49 | 2293.4 | 2202.3 | 2530.8 | 3663.6 |
| 2 | 270/300 | 351232 | 152.05 | 6.49 | 2310.0 | 2215.5 | 2498.2 | 3664.2 |
| 3 | 273/300 | 364472 | 146.43 | 6.49 | 2489.0 | 2383.4 | 2563.4 | 3667.8 |
| 4 | 273/300 | 363328 | 139.23 | 6.49 | 2609.5 | 2493.3 | 2574.3 | 3713.5 |

Legacy long-CP arm A control, delivered application bits per 300 frame slots.
These ratios are not comparable to the airtime ratios above.

| Seed | Adaptive bits | Best fixed bits | Oracle bits | a/bf | a/oracle |
|---|---:|---:|---:|---:|---:|
| 0 | 340968 | 371664 | 436712 | 0.917409273 | 0.780761692 |
| 1 | 349184 | 370240 | 436104 | 0.943128781 | 0.800689744 |
| 2 | 344040 | 364544 | 437400 | 0.943754389 | 0.786556927 |
| 3 | 348440 | 377360 | 438248 | 0.923362307 | 0.795074935 |
| 4 | 346376 | 370240 | 439312 | 0.935544512 | 0.788451033 |

Aggregate decision output, x0 then x1:

```text
arm B / best-fixed(joint) = 0.948 (x0) / 0.908 (x1)  (bar: > 1.0)
arm B / oracle(joint)     = 0.654 (x0) / 0.627 (x1)  (bar: >= 0.8)
```

Existing delay-spread segment output, retained as descriptive context only.
It does not measure fading-segment delivered-goodput non-regression.

```text
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s0 awgn ramp down      98       0       0       0       0       0       0       0         2         0
s0 watterson good      27      10       0       0       0       0       0       0        13         0
s0 watterson poor      23       0       0       0       0      15       0       0        12         0
s1 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s1 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s1 watterson good      23      14       0       0       0       0       0       0        13         0
s1 watterson poor      18       0       0       0       0      22       0       0        10         0
s2 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s2 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s2 watterson good      30       5       0       0       0       0       0       0        15         0
s2 watterson poor      14       0       0       0       0      22       0       0        14         0
s3 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s3 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s3 watterson good      32       4       0       0       0       0       0       0        14         0
s3 watterson poor      22       0       0       0       0      16       0       0        12         0
s4 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s4 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s4 watterson good      24      11       0       0       0       0       0       0        15         0
s4 watterson poor      17       0       0       0       0      22       0       0        11         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)

```

The standard aggregate x1 bar also fails. There is no supported claim of a
paired fading-tail gain, reduced volatility or new physical ceiling. COP-133
remains open on its intended outcome; the retained work is reconstruction,
measurement validation, receive-method equivalence and the negative applicability
record. The ignored grid took 253.76 seconds in this container; this experiment
runtime is not a production receive CPU benchmark.

## Synthetic transfer impairments (COP-132)

Run the independent-frame diagnostic with:

```sh
cargo run -q -p coppa-bench --example impairment_transfer -- --level 2 --frames 2 --trials 1 --snr 30 --seed 12648430 --impairment all
```

Measured output on 2026-11-04, one trial with seed 12648430:

```csv
phy,level,channel,impairment,parameters,snr_db,frames,trials,seed,recovery_fraction,goodput_bps,airtime_s
v1-independent,2,awgn,none-v1,none,30.0,2,1,12648430,1.000000,685.71,2.730
v1-independent,2,awgn,impulsive-v1,p=0.001;amplitude_rms=10,30.0,2,1,12648430,1.000000,685.71,2.730
v1-independent,2,awgn,clipping-v1,threshold_rms=2,30.0,2,1,12648430,1.000000,685.71,2.730
v1-independent,2,awgn,agc-v1,fade_hz=1;fade_db=20;target_rms=0.5;block_samples=480;beta_attack=0.6;beta_release=0.85;gain_min=0.0001;gain_max=10000,30.0,2,1,12648430,1.000000,685.71,2.730
v1-independent,2,awgn,tilt-v1,low_hz=300;high_hz=2700;low_db=-6;high_db=6,30.0,2,1,12648430,1.000000,685.71,2.730

```

Defaults are level 2, two frames, ten trials, SNR 30 dB, seed 12648430,
channel `awgn` and impairment `all`. Levels 1 through 9 are accepted.
`--channel awgn|good|moderate|poor` selects AWGN or Watterson plus AWGN.
`--impairment all|none|impulsive|clipping|agc|tilt` selects all five rows in
that order, or one row. SNR is one finite signed value per invocation. Frames
and trials must be positive. Successful stdout contains only CSV; invalid
arguments fail before encoding and produce no CSV. Reports are collected before
stdout is written. No file is written into `results/`.

Preset labels are versioned. `none-v1` is the control. `impulsive-v1` adds
independent signed impulses with probability 0.001 per sample and magnitude ten
times clean RMS. `clipping-v1` symmetrically limits to twice clean RMS, bypassing
clipping for zero clean power. `agc-v1` stresses tracking with a 1 Hz, 20 dB
sinusoidal fade and fresh block AGC targeting 0.5 RMS every 480 samples, using
attack 0.6, release 0.85 and gain limits 0.0001 through 10000. `tilt-v1` shapes
300 through 2700 Hz from -6 to +6 dB, with unity at 1500 Hz and constant gains
outside that band. It is zero-phase finite-buffer FFT shaping with circular
boundaries, not a causal rig filter.

The encoder's frame buffers are concatenated. Clean power is measured first.
Tilt or the AGC stress fade precedes optional Watterson fading. AWGN follows,
referenced to the original clean power in 3000 Hz at 48000 samples/s. Impulses,
clipping or actual AGC follow noise. State continues across frame boundaries;
the buffer is then split at its original lengths. Effects are not normalized
away. Trials use seed plus trial index, with fixed XOR salts for Watterson
(0x3333333333333333), AWGN (0x5555555555555555) and impulses
(0x7777777777777777). Payloads and baseline channel draws are paired across cases.

Every frame uses a fresh receiver, including the control, so failed decodes
cannot carry HARQ state into unrelated frames, trials or cases. The legacy
`transfer_baseline` decoder policy is unchanged; its numbers are not directly
comparable as an impairment-only difference.

Recovery is the mean fraction of matching payload bytes, including accidental
matches in zero-filled failed frames. It is neither FER nor authenticated
delivery. `airtime_s` is the sum of encoded frame samples divided by 48000.
`goodput_bps` is sent payload bytes times eight times mean recovery divided by
that airtime. Airtime excludes ARQ, turnaround, CPU time and session overhead.
Failed transfers still have rows. These small synthetic runs impose no minimum
recovery threshold and make no confidence-bound or real-radio performance claim.
Physical rig/ALC behavior and group-delay ripple remain unmodeled.

## COP-152: Bounded-prefix payload residual
The CRC-verified measurement accepts full padded observations or exactly one missing final payload OFDM symbol. It requires at least two observed symbols and coverage of every physical data carrier in both alternating pilot phases. Missing coded positions contribute neither error nor count. Unused modulation padding is excluded. Non-finite input, partial symbols, malformed maps and larger truncations are rejected. Turbo rescues keep the matching pilot recommendation. Reconstruction uses the current RV, including when CRC succeeds after HARQ combining.
The tested candidate carried the residual recommendation through the shared receive result. Production adoption was withdrawn after the full-loop failure. Both public wrappers now share the pilot recommendation. Payload decoding, SNR telemetry, LDPC noise, filtering, waveform, controller dwell and probing cadence retain their prior meanings.
### Frozen screening
Command: `cargo test -p coppa-protocol --release --lib residual_recommendation_grid -- --ignored --nocapture`. The uncalibrated per-carrier residual and existing two-dimensional selector were frozen before evaluating the held-out partition. No residual calibration was fitted. Each profile/partition has 1440 transmissions over all nine levels, AWGN/Good/Moderate/Poor, SNRs 6/12/18/24/30 dB and eight trials. Training seeds are `0xCA11B + trial`; held-out seeds are `0x5A1AD + trial`. Final seeds 0 through 4 are separate.
| Profile | Partition | CRC successes | Available | Sum pilot selected bps | Sum candidate selected bps |
|---|---|---:|---:|---:|---:|
| robust | ca11b | 662 | 643 | 1280676.343315449 | 1243018.591445306 |
| robust | 5a1ad | 931 | 910 | 1742267.836166123 | 1848329.743018487 |
| standard | ca11b | 788 | 770 | 1597427.134269239 | 1606196.098170834 |
| standard | 5a1ad | 768 | 743 | 1532911.852591848 | 1537790.617335875 |

Both held-out profiles improve, satisfying the screening prerequisite for integration. Robust training declines, so this result does not establish consistent improvement across seeds. These sums condition on CRC-successful probe frames. A selected transmission that failed decoding contributes zero goodput. They are screening scores, not closed-loop acceptance or fading-segment non-regression.
Coverage below is CRC-successes/available for each level L1 through L9. Each level has 40 attempted transmissions per preset and partition. CRC-success conditioning remains a limitation; unavailable successes use pilot fallback.
| Profile | Partition | Preset | L1 | L2 | L3 | L4 | L5 | L6 | L7 | L8 | L9 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| robust | ca11b | None | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 36/34 | 30/27 |
| robust | ca11b | Some(Good) | 40/40 | 40/40 | 21/21 | 15/15 | 0/0 | 10/9 | 4/4 | 0/0 | 0/0 |
| robust | ca11b | Some(Moderate) | 40/40 | 35/35 | 14/14 | 6/6 | 5/5 | 10/10 | 0/0 | 0/0 | 0/0 |
| robust | ca11b | Some(Poor) | 33/28 | 23/15 | 10/10 | 0/0 | 5/5 | 5/5 | 0/0 | 0/0 | 0/0 |
| robust | 5a1ad | None | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 39/37 | 31/30 |
| robust | 5a1ad | Some(Good) | 40/40 | 35/35 | 30/30 | 20/20 | 20/20 | 34/34 | 17/17 | 0/0 | 0/0 |
| robust | 5a1ad | Some(Moderate) | 40/35 | 35/35 | 21/21 | 16/16 | 20/20 | 34/34 | 17/13 | 4/4 | 0/0 |
| robust | 5a1ad | Some(Poor) | 35/35 | 35/33 | 33/28 | 19/19 | 20/20 | 29/29 | 19/18 | 8/7 | 0/0 |
| standard | ca11b | None | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 32/32 |
| standard | ca11b | Some(Good) | 40/40 | 40/40 | 35/35 | 22/22 | 18/18 | 11/11 | 9/8 | 0/0 | 0/0 |
| standard | ca11b | Some(Moderate) | 40/40 | 40/40 | 25/25 | 15/15 | 10/10 | 8/6 | 0/0 | 0/0 | 0/0 |
| standard | ca11b | Some(Poor) | 40/40 | 40/30 | 15/15 | 10/10 | 8/8 | 10/5 | 0/0 | 0/0 | 0/0 |
| standard | 5a1ad | None | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 40/40 | 32/32 |
| standard | 5a1ad | Some(Good) | 40/40 | 40/40 | 31/31 | 17/17 | 15/15 | 10/10 | 0/0 | 3/2 | 0/0 |
| standard | 5a1ad | Some(Moderate) | 35/31 | 35/32 | 31/31 | 20/17 | 10/10 | 9/9 | 0/0 | 0/0 | 0/0 |
| standard | 5a1ad | Some(Poor) | 30/30 | 30/25 | 19/15 | 5/5 | 14/13 | 10/10 | 4/3 | 8/5 | 0/0 |

### Full controls and execution

The diagnostic runs independent seeds concurrently and joins them in seed order. Each seed still owns its transceivers, schedule and RNGs. Baseline source is `7f224a3e902d552a3406e69e132d14832cb1d30e` with only the diagnostic and acceptance-arithmetic changes copied into an isolated build. The production candidate is absent from that baseline. The same two full commands run on baseline and candidate, without smoke overrides:

```sh
cargo run -p coppa-bench --release --example closed_loop_arq
cargo run -p coppa-bench --release --example closed_loop_arq -- standard
```

The robust short-CP profile remains synthetic. Standard uses the production pair. Primary standard acceptance uses arm B x1 and all 18 comparator cells; x0 remains descriptive. Nominal handshake charging assumes loss-free control delivery and retains the existing optimistic limits. Segment airtime includes failed data transmissions and assigns each handshake to its deciding frame's segment.

The 1944-expected/1908-observed regression first failed with the old helper
(`residual_accepts_bounded_current_frame_prefix`, exit 101), then passed with
the bounded helper. Hand-calculated pooling/count tests and invalid-geometry
checks pass. Reconstruction fixtures cover both profiles, levels 1 through 9,
RVs 0 through 3, lengths zero/one/maximum and one/two/eight codewords.
The training regression `training_fixture_residual_choice_delivers_more_goodput`
transmits and decodes both selected choices. It verifies pilot L7 versus residual
L8 at robust AWGN 12 dB, training seed `0xCA11B`, probe L5.
Wrapper agreement covers RVs 0 through 3 on independent receivers.
`harq_success_measures_current_rv_symbols` checks an RV1 success with an existing
RV0 contribution. `turbo_rescue_retains_pilot_recommendation` checks a real rescue
against its re-equalized noise vector. These are implementation checks, not proof
of the full-loop acceptance bars.

The rejected production candidate added payload reconstruction through the existing
LDPC encode/rate-match/interleave/map chain, physical-carrier residual sums and
counts, and capacity conversion. That candidate receive core also copied the current
observation vector before decode, including on failures. Added buffers include
that observation copy, reconstructed constellation symbols, two carrier maps,
carrier sums/counts, residual means and their capacity-floor copy. All are bounded
by existing frame geometry. No cross-frame measurement state is retained.
The full closed-loop diagnostic does not expose per-seed residual availability;
coverage is measured separately in the grid above to preserve the public API.

CPU corpus: compile each release test binary first, then invoke it directly as
`coppa_protocol-<hash> receive_cpu_corpus --ignored --nocapture`. The corpus
receives the same clean robust L2 eight-byte payload 500 times and checks every
payload. POSIX child resource usage, excluding compilation, was baseline
40.407444 user + 0.374556 system seconds and candidate 39.511223 user + 0.370224
system seconds. Wall times were 148.989658 and 142.010418 seconds respectively.
Both exited 0. These single samples ran under concurrent benchmark load; they do
not resolve the extra reconstruction cost from run-to-run timing variation and
are not evidence of a CPU speedup. The earlier cargo-invocation timing included
compilation on the candidate and was discarded.

### Full-run results

#### Robust

All 90 fixed-cell results and all five unrounded joint comparator values match baseline and candidate.

| Variant | Seed | B delivered bits | Data air s | Switch air s | B x1 bps | Joint best fixed bps | Joint oracle bps |
|---|---:|---:|---:|---:|---:|---:|---:|
| baseline | 0 | 324096 | 176.55524999999975 | 7.60975 | 1759.8132109792873 | 2240.662525879909 | 3258.1447162773043 |
| baseline | 1 | 341520 | 163.97425000000015 | 7.60975 | 1990.3953748601252 | 2230.8350586611377 | 3218.2865690479243 |
| baseline | 2 | 323128 | 190.79225000000002 | 7.60975 | 1628.652936966361 | 2329.109730848853 | 3303.837074380825 |
| baseline | 3 | 334648 | 173.22025000000014 | 7.60975 | 1850.6221312835246 | 2299.6273291925386 | 3259.7881633716934 |
| baseline | 4 | 329680 | 178.02724999999987 | 7.60975 | 1775.9390638719665 | 2221.007591442366 | 3247.7859064410522 |

baseline robust-style seed-0 legacy long-CP slot counts are adaptive 330144, fixed 358848, oracle 429256. Ratios 0.920010700910 and 0.769107478987; predicates `330144 > 358848` = False and `5 * 330144 >= 4 * 429256` = False. This is the historical acceptance comparison.

baseline arm B x1 sums of per-seed goodputs are [9005.422717961264, 11321.242236024804, 16287.8424295188], adaptive/fixed/oracle. Ratios of means are 0.795444751576 and 0.552892303381; >1.0 = False, >=0.8 = False. Robust synthetic CP results are descriptive.
| Variant | Seed | B delivered bits | Data air s | Switch air s | B x1 bps | Joint best fixed bps | Joint oracle bps |
|---|---:|---:|---:|---:|---:|---:|---:|
| candidate | 0 | 313040 | 181.3392499999997 | 7.60975 | 1656.7433540267507 | 2240.662525879909 | 3258.1447162773043 |
| candidate | 1 | 322712 | 165.14724999999993 | 7.60975 | 1868.0111370306277 | 2230.8350586611377 | 3218.2865690479243 |
| candidate | 2 | 312432 | 186.97424999999993 | 7.60975 | 1605.6407515520295 | 2329.109730848853 | 3303.837074380825 |
| candidate | 3 | 321184 | 171.88625 | 7.60975 | 1789.3657797388244 | 2299.6273291925386 | 3259.7881633716934 |
| candidate | 4 | 327080 | 175.06024999999968 | 7.60975 | 1790.5512673126434 | 2221.007591442366 | 3247.7859064410522 |

candidate robust-style seed-0 legacy long-CP slot counts are adaptive 314368, fixed 358848, oracle 429256. Ratios 0.876047797396 and 0.732355517453; predicates `314368 > 358848` = False and `5 * 314368 >= 4 * 429256` = False. This is the historical acceptance comparison.

candidate arm B x1 sums of per-seed goodputs are [8710.312289660877, 11321.242236024804, 16287.8424295188], adaptive/fixed/oracle. Ratios of means are 0.769377786295 and 0.534773855245; >1.0 = False, >=0.8 = False. Robust synthetic CP results are descriptive.

#### Standard

All 90 fixed-cell results and all five unrounded joint comparator values match baseline and candidate.

| Variant | Seed | B delivered bits | Data air s | Switch air s | B x1 bps | Joint best fixed bps | Joint oracle bps |
|---|---:|---:|---:|---:|---:|---:|---:|
| baseline | 0 | 350192 | 156.3469999999996 | 6.487 | 2150.6073670118085 | 2491.3623188405863 | 3618.2186707233354 |
| baseline | 1 | 353472 | 157.2439999999996 | 6.487 | 2158.8581270498617 | 2519.9694889397483 | 3658.3252238956748 |
| baseline | 2 | 344648 | 155.21999999999983 | 6.487 | 2131.311569690863 | 2498.2456140350955 | 3662.812316559057 |
| baseline | 3 | 356576 | 146.80199999999962 | 6.487 | 2326.168218202225 | 2563.4172387490544 | 3665.815170102382 |
| baseline | 4 | 363328 | 139.2349999999998 | 6.487 | 2493.295452985826 | 2563.4172387490544 | 3704.09838739494 |

baseline robust-style seed-0 legacy long-CP slot counts are adaptive 340968, fixed 371664, oracle 436712. Ratios 0.917409272892 and 0.780761691916; predicates `340968 > 371664` = False and `5 * 340968 >= 4 * 436712` = False. Standard legacy is descriptive, not historical acceptance.

baseline arm B x1 sums of per-seed goodputs are [11260.240734940584, 12636.411899313538, 18309.26976867539], adaptive/fixed/oracle. Ratios of means are 0.891094784236 and 0.615002175248; >1.0 = False, >=0.8 = False. This is the production-profile acceptance comparison.
| Variant | Seed | B delivered bits | Data air s | Switch air s | B x1 bps | Joint best fixed bps | Joint oracle bps |
|---|---:|---:|---:|---:|---:|---:|---:|
| candidate | 0 | 352000 | 158.8999999999996 | 6.487 | 2128.341405309975 | 2491.3623188405863 | 3618.2186707233354 |
| candidate | 1 | 351152 | 150.68899999999962 | 6.487 | 2234.1324375222734 | 2519.9694889397483 | 3658.3252238956748 |
| candidate | 2 | 352984 | 168.83599999999961 | 6.487 | 2013.3353866862922 | 2498.2456140350955 | 3662.812316559057 |
| candidate | 3 | 359048 | 156.69199999999964 | 6.487 | 2200.332150583107 | 2563.4172387490544 | 3665.815170102382 |
| candidate | 4 | 374528 | 138.4989999999997 | 6.487 | 2583.201136661476 | 2563.4172387490544 | 3704.09838739494 |

candidate robust-style seed-0 legacy long-CP slot counts are adaptive 353576, fixed 371664, oracle 436712. Ratios 0.951332386241 and 0.809631977138; predicates `353576 > 371664` = False and `5 * 353576 >= 4 * 436712` = True. Standard legacy is descriptive, not historical acceptance.

candidate arm B x1 sums of per-seed goodputs are [11159.342516763123, 12636.411899313538, 18309.26976867539], adaptive/fixed/oracle. Ratios of means are 0.883110063654 and 0.609491402866; >1.0 = False, >=0.8 = False. This is the production-profile acceptance comparison.

#### Standard fading segments

Pooled goodput is total delivered bits divided by all data plus switch airtime across the five seeds.

| Variant | Segment | Bits | Data air s | Switch air s | Pooled x1 bps |
|---|---|---:|---:|---:|---:|
| baseline | Good | 186848 | 174.11000000000007 | 0 | 1073.160645568893 |
| candidate | Good | 175744 | 161.36800000000008 | 0 | 1089.088295077090 |

Good non-regression = True, change 1.484181%.
| Variant | Segment | Bits | Data air s | Switch air s | Pooled x1 bps |
|---|---|---:|---:|---:|---:|
| baseline | Poor | 184472 | 189.88800000000006 | 0 | 971.477923828783 |
| candidate | Poor | 173088 | 222.59399999999997 | 0 | 777.595083425429 |

Poor non-regression = False, change -19.957514%.

Full acceptance outcomes: {'historical': False, 'standard': False, 'nonregression': False}. Screening gains do not replace these checks. No final seeds were used for calibration and no parameters were changed during these runs.

<details>
<summary>Complete baseline robust output</summary>

```text
   Compiling autocfg v1.5.0
   Compiling proc-macro2 v1.0.106
   Compiling unicode-ident v1.0.24
   Compiling quote v1.0.45
   Compiling strength_reduce v0.2.4
   Compiling serde_core v1.0.229
   Compiling libc v0.2.191
   Compiling getrandom v0.4.2
   Compiling cfg-if v1.0.4
   Compiling utf8parse v0.2.2
   Compiling num-traits v0.2.19
   Compiling rand_core v0.10.1
   Compiling thiserror v2.0.21
   Compiling anstyle-parse v1.0.0
   Compiling anstyle-query v1.1.5
   Compiling crc-catalog v2.4.0
   Compiling once_cell v1.21.4
   Compiling cpufeatures v0.3.0
   Compiling syn v3.0.3
   Compiling syn v2.0.117
   Compiling anstyle v1.0.14
   Compiling anyhow v1.0.104
   Compiling num-integer v0.1.46
   Compiling num-complex v0.4.6
   Compiling colorchoice v1.0.5
   Compiling is_terminal_polyfill v1.70.2
   Compiling anstream v1.0.0
   Compiling transpose v0.2.3
   Compiling primal-check v0.3.4
   Compiling rustfft v6.4.1
   Compiling tracing-core v0.1.36
   Compiling chacha20 v0.10.1
   Compiling crc v3.4.0
   Compiling winnow v1.0.0
   Compiling serde v1.0.229
   Compiling heck v0.5.0
   Compiling clap_lex v1.1.0
   Compiling twox-hash v2.1.2
   Compiling pin-project-lite v0.2.17
   Compiling strsim v0.11.1
   Compiling lz4_flex v0.14.0
   Compiling toml_parser v1.1.5+spec-1.1.0
   Compiling clap_builder v4.6.7
   Compiling toml_datetime v1.1.2+spec-1.1.0
   Compiling serde_spanned v1.1.2
   Compiling rand v0.10.3
   Compiling thiserror-impl v2.0.21
   Compiling clap_derive v4.6.7
   Compiling serde_derive v1.0.229
   Compiling tracing-attributes v0.1.31
   Compiling tracing v0.1.44
   Compiling toml_writer v1.1.3+spec-1.1.0
   Compiling hound v3.5.1
   Compiling rtrb v0.4.1
   Compiling toml v1.1.8+spec-1.1.0
   Compiling coppa-dsp v0.7.0 (/tmp/cop152-baseline/crates/coppa-dsp)
   Compiling realfft v3.5.1
   Compiling rubato v0.16.2
   Compiling coppa-ml v0.7.0 (/tmp/cop152-baseline/crates/coppa-ml)
   Compiling coppa-codec v0.7.0 (/tmp/cop152-baseline/crates/coppa-codec)
   Compiling coppa-protocol v0.7.0 (/tmp/cop152-baseline/crates/coppa-protocol)
   Compiling coppa-channel v0.7.0 (/tmp/cop152-baseline/crates/coppa-channel)
   Compiling coppa-audio v0.7.0 (/tmp/cop152-baseline/crates/coppa-audio)
   Compiling clap v4.6.7
   Compiling coppa-bench v0.7.0 (/tmp/cop152-baseline/crates/coppa-bench)
    Finished `release` profile [optimized] target(s) in 16.88s
     Running `/tmp/cop152-baseline/target/release/examples/closed_loop_arq`
=== Closed-loop adaptive rate + CP contrast (COP-2) ===
base pair    : hf_robust <-> synthetic robust short-CP (metric question, D1c)
frames/run   : 300   seeds: [0, 1, 2, 3, 4]   runs: 120
NOTE         : the short-CP half of this pair is a BENCH-LOCAL SYNTHETIC profile with an
unallocated bandwidth_id (5). `select_ofdm_profile` can never select it and the
engine can never negotiate it, so this base measures the CP lever's MAGNITUDE,
not a shippable feature. Use the `standard` base for the shippable claim.

  seed 4 fixed LongCp L1: 293/300 delivered, 273.3 bps
  seed 2 fixed LongCp L1: 294/300 delivered, 274.2 bps
  seed 1 fixed LongCp L1: 299/300 delivered, 278.8 bps
  seed 0 fixed LongCp L1: 295/300 delivered, 275.1 bps
  seed 3 fixed LongCp L1: 297/300 delivered, 277.0 bps
  seed 1 fixed LongCp L2: 287/300 delivered, 559.2 bps
  seed 4 fixed LongCp L2: 282/300 delivered, 549.5 bps
  seed 2 fixed LongCp L2: 285/300 delivered, 555.3 bps
  seed 0 fixed LongCp L2: 283/300 delivered, 551.4 bps
  seed 3 fixed LongCp L2: 283/300 delivered, 551.4 bps
  seed 1 fixed LongCp L3: 272/300 delivered, 950.9 bps
  seed 3 fixed LongCp L3: 271/300 delivered, 947.4 bps
  seed 4 fixed LongCp L3: 279/300 delivered, 975.3 bps
  seed 2 fixed LongCp L3: 277/300 delivered, 968.3 bps
  seed 0 fixed LongCp L3: 281/300 delivered, 982.3 bps
  seed 1 fixed LongCp L4: 246/300 delivered, 1308.3 bps
  seed 2 fixed LongCp L4: 258/300 delivered, 1372.1 bps
  seed 4 fixed LongCp L4: 256/300 delivered, 1361.5 bps
  seed 3 fixed LongCp L4: 246/300 delivered, 1308.3 bps
  seed 0 fixed LongCp L4: 252/300 delivered, 1340.2 bps
  seed 1 fixed LongCp L5: 236/300 delivered, 1515.2 bps
  seed 2 fixed LongCp L5: 235/300 delivered, 1508.8 bps
  seed 4 fixed LongCp L5: 245/300 delivered, 1573.0 bps
  seed 3 fixed LongCp L5: 244/300 delivered, 1566.6 bps
  seed 0 fixed LongCp L5: 240/300 delivered, 1540.9 bps
  seed 1 fixed LongCp L6: 245/300 delivered, 1386.7 bps
  seed 2 fixed LongCp L6: 245/300 delivered, 1386.7 bps
  seed 3 fixed LongCp L6: 250/300 delivered, 1415.0 bps
  seed 4 fixed LongCp L6: 250/300 delivered, 1415.0 bps
  seed 0 fixed LongCp L6: 238/300 delivered, 1347.0 bps
  seed 1 fixed LongCp L7: 224/300 delivered, 1928.8 bps
  seed 3 fixed LongCp L7: 229/300 delivered, 1971.9 bps
  seed 2 fixed LongCp L7: 222/300 delivered, 1911.6 bps
  seed 0 fixed LongCp L7: 226/300 delivered, 1946.0 bps
  seed 4 fixed LongCp L7: 226/300 delivered, 1946.0 bps
  seed 3 fixed LongCp L8: 194/300 delivered, 1946.2 bps
  seed 1 fixed LongCp L8: 196/300 delivered, 1966.2 bps
  seed 2 fixed LongCp L8: 189/300 delivered, 1896.0 bps
  seed 4 fixed LongCp L8: 192/300 delivered, 1926.1 bps
  seed 0 fixed LongCp L8: 193/300 delivered, 1936.1 bps
  seed 3 fixed LongCp L9: 122/300 delivered, 1533.7 bps
  seed 1 fixed LongCp L9: 126/300 delivered, 1584.0 bps
  seed 2 fixed LongCp L9: 122/300 delivered, 1533.7 bps
  seed 4 fixed LongCp L9: 121/300 delivered, 1521.1 bps
  seed 0 fixed LongCp L9: 122/300 delivered, 1533.7 bps
  seed 3 fixed ShortCp L1: 295/300 delivered, 314.0 bps
  seed 1 fixed ShortCp L1: 291/300 delivered, 309.7 bps
  seed 0 fixed ShortCp L1: 296/300 delivered, 315.1 bps
  seed 4 fixed ShortCp L1: 295/300 delivered, 314.0 bps
  seed 2 fixed ShortCp L1: 291/300 delivered, 309.7 bps
  seed 3 fixed ShortCp L2: 291/300 delivered, 647.1 bps
  seed 1 fixed ShortCp L2: 282/300 delivered, 627.1 bps
  seed 0 fixed ShortCp L2: 287/300 delivered, 638.2 bps
  seed 2 fixed ShortCp L2: 278/300 delivered, 618.2 bps
  seed 4 fixed ShortCp L2: 291/300 delivered, 647.1 bps
  seed 3 fixed ShortCp L3: 282/300 delivered, 1125.1 bps
  seed 1 fixed ShortCp L3: 282/300 delivered, 1125.1 bps
  seed 0 fixed ShortCp L3: 278/300 delivered, 1109.2 bps
  seed 2 fixed ShortCp L3: 275/300 delivered, 1097.2 bps
  seed 4 fixed ShortCp L3: 286/300 delivered, 1141.1 bps
  seed 3 fixed ShortCp L4: 267/300 delivered, 1620.7 bps
  seed 1 fixed ShortCp L4: 264/300 delivered, 1602.5 bps
  seed 0 fixed ShortCp L4: 255/300 delivered, 1547.8 bps
  seed 2 fixed ShortCp L4: 255/300 delivered, 1547.8 bps
  seed 4 fixed ShortCp L4: 271/300 delivered, 1644.9 bps
  seed 1 fixed ShortCp L5: 250/300 delivered, 1831.9 bps
  seed 3 fixed ShortCp L5: 250/300 delivered, 1831.9 bps
  seed 2 fixed ShortCp L5: 255/300 delivered, 1868.5 bps
  seed 0 fixed ShortCp L5: 248/300 delivered, 1817.2 bps
  seed 4 fixed ShortCp L5: 249/300 delivered, 1824.6 bps
  seed 1 fixed ShortCp L6: 251/300 delivered, 1621.4 bps
  seed 3 fixed ShortCp L6: 255/300 delivered, 1647.2 bps
  seed 2 fixed ShortCp L6: 260/300 delivered, 1679.5 bps
  seed 0 fixed ShortCp L6: 250/300 delivered, 1614.9 bps
  seed 4 fixed ShortCp L6: 254/300 delivered, 1640.7 bps
  seed 1 fixed ShortCp L7: 227/300 delivered, 2230.8 bps
  seed 3 fixed ShortCp L7: 234/300 delivered, 2299.6 bps
  seed 2 fixed ShortCp L7: 237/300 delivered, 2329.1 bps
  seed 0 fixed ShortCp L7: 228/300 delivered, 2240.7 bps
  seed 4 fixed ShortCp L7: 226/300 delivered, 2221.0 bps
  seed 1 fixed ShortCp L8: 190/300 delivered, 2175.4 bps
  seed 3 fixed ShortCp L8: 196/300 delivered, 2244.1 bps
  seed 2 fixed ShortCp L8: 196/300 delivered, 2244.1 bps
  seed 0 fixed ShortCp L8: 192/300 delivered, 2198.3 bps
  seed 4 fixed ShortCp L8: 190/300 delivered, 2175.4 bps
  seed 1 fixed ShortCp L9: 123/300 delivered, 1764.8 bps
  seed 3 fixed ShortCp L9: 124/300 delivered, 1779.1 bps
  seed 2 fixed ShortCp L9: 124/300 delivered, 1779.1 bps
  seed 0 fixed ShortCp L9: 119/300 delivered, 1707.4 bps
  seed 4 fixed ShortCp L9: 124/300 delivered, 1779.1 bps
  seed 1 arm A  : 1515.8 bps
  seed 3 arm A  : 1536.1 bps
  seed 2 arm A  : 1519.6 bps
  seed 0 arm A  : 1484.9 bps
  seed 4 arm A  : 1508.2 bps
  seed 1 arm C  : 2101.1 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 3 arm C  : 1948.0 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 2 arm C  : 1706.4 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 0 arm C  : 1850.7 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 4 arm C  : 1866.9 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 1 arm C' : 2122.4 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 3 arm C' : 1870.2 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 2 arm C' : 1898.3 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 0 arm C' : 1850.7 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 4 arm C' : 1868.7 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 1 arm B  : 2082.8 bps, 1 switches
  seed 3 arm B  : 1931.9 bps, 1 switches
  seed 2 arm B  : 1693.6 bps, 1 switches
  seed 0 arm B  : 1835.7 bps, 1 switches
  seed 4 arm B  : 1851.9 bps, 1 switches
  seed 1 arm B0 : 2090.9 bps, 1 switches
  seed 3 arm B0 : 1939.1 bps, 1 switches
  seed 0 arm B0 : 1842.3 bps, 1 switches
  seed 2 arm B0 : 1699.3 bps, 1 switches
  seed 4 arm B0 : 1858.5 bps, 1 switches
  seed 1 arm P  : 1515.8 bps (1 rebuilds; arm B had 1)
  seed 3 arm P  : 1536.1 bps (1 rebuilds; arm B had 1)
  seed 2 arm P  : 1519.6 bps (1 rebuilds; arm B had 1)
  seed 0 arm P  : 1484.9 bps (1 rebuilds; arm B had 1)
  seed 4 arm P  : 1508.2 bps (1 rebuilds; arm B had 1)

=== PRIMARY: airtime-normalized goodput (bits/s) ===
The `airtime(s)` and `goodput` columns are DATA-AIRTIME-ONLY (= switch cost x0) for EVERY
arm, arm B included. Arm B's CP-handshake air is real and is charged separately in the
SWITCH ACCOUNTING block below (and as the x1 half of every ratio in THE BAR); it is NOT in
this table's denominator. Arms A / P / B0 / C / C' and all 18 comparator cells have no
handshake air at all, so for them x0 is the whole story.
seed   arm       deliv       bits  airtime(s)    goodput     FER   delivery 95% CI
0      A      264/300      330144      222.34     1484.9   12.0%   0.838-0.912
0      P      264/300      330144      222.34     1484.9   12.0%   0.838-0.912
0      B      255/300      324096      176.56     1835.7   15.0%   0.805-0.886
0      B0     255/300      324096      175.91     1842.3   15.0%   0.805-0.886
0      C      255/300      324096      175.12     1850.7   15.0%   0.805-0.886
0      C'     255/300      324096      175.12     1850.7   15.0%   0.805-0.886
0      C(fix)  228/300      324672      144.90     2240.7   24.0%   0.709-0.805    (L7)
1      A      261/300      325680      214.86     1515.8   13.0%   0.827-0.903
1      P      261/300      325680      214.86     1515.8   13.0%   0.827-0.903
1      B      258/300      341520      163.97     2082.8   14.0%   0.816-0.895
1      B0     258/300      341520      163.33     2090.9   14.0%   0.816-0.895
1      C      258/300      341520      162.54     2101.1   14.0%   0.816-0.895
1      C'     257/300      340096      160.24     2122.4   14.3%   0.812-0.892
1      C(fix)  227/300      323248      144.90     2230.8   24.3%   0.705-0.802    (L7)
2      A      252/300      318512      209.61     1519.6   16.0%   0.794-0.877
2      P      252/300      318512      209.61     1519.6   16.0%   0.794-0.877
2      B      258/300      323128      190.79     1693.6   14.0%   0.816-0.895
2      B0     258/300      323128      190.15     1699.3   14.0%   0.816-0.895
2      C      258/300      323128      189.36     1706.4   14.0%   0.816-0.895
2      C'     260/300      333128      175.49     1898.3   13.3%   0.824-0.901
2      C(fix)  237/300      337488      144.90     2329.1   21.0%   0.740-0.832    (L7)
3      A      260/300      325208      211.71     1536.1   13.3%   0.824-0.901
3      P      260/300      325208      211.71     1536.1   13.3%   0.824-0.901
3      B      263/300      334648      173.22     1931.9   12.3%   0.835-0.909
3      B0     263/300      334648      172.58     1939.1   12.3%   0.835-0.909
3      C      263/300      334648      171.79     1948.0   12.3%   0.835-0.909
3      C'     258/300      325096      173.83     1870.2   14.0%   0.816-0.895
3      C(fix)  234/300      333216      144.90     2299.6   22.0%   0.730-0.823    (L7)
4      A      261/300      323888      214.75     1508.2   13.0%   0.827-0.903
4      P      261/300      323888      214.75     1508.2   13.0%   0.827-0.903
4      B      258/300      329680      178.03     1851.9   14.0%   0.816-0.895
4      B0     258/300      329680      177.39     1858.5   14.0%   0.816-0.895
4      C      258/300      329680      176.59     1866.9   14.0%   0.816-0.895
4      C'     258/300      330000      176.59     1868.7   14.0%   0.816-0.895
4      C(fix)  226/300      321824      144.90     2221.0   24.7%   0.702-0.799    (L7)

=== COMPARATORS: 9-cell long-CP-only vs 18-cell joint level x CP (D4) ===
seed    bestfix(long)    lvl bestfix(joint)   cell  oracle(long) oracle(joint)
0              1946.0     L7         2240.7 shortL7        2732.1        3258.1
1              1966.2     L8         2230.8 shortL7        2753.7        3218.3
2              1911.6     L7         2329.1 shortL7        2708.8        3303.8
3              1971.9     L7         2299.6 shortL7        2749.7        3259.8
4              1946.0     L7         2221.0 shortL7        2750.0        3247.8

=== THE BAR (adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8) ===
Each ratio is printed at BOTH switch-cost multipliers: x0 (arm B's data airtime only) and
x1 (the loss-free nominal, one handshake charged per switch). x0 alone is what these rows
used to report, unlabelled -- an arm B that paid nothing for its own switches.
seed          B/bf(long)       B/orc(long)       B/bf(joint)      B/orc(joint)
0            0.943/0.904       0.672/0.644       0.819/0.785       0.563/0.540
1            1.059/1.012       0.756/0.723       0.934/0.892       0.647/0.618
2            0.886/0.852       0.625/0.601       0.727/0.699       0.513/0.493
3            0.980/0.939       0.703/0.673       0.840/0.805       0.593/0.568
4            0.952/0.913       0.673/0.646       0.834/0.800       0.570/0.547
(each cell is x0/x1)

REMINDER (Key Discovery 1): a uniform CP change is ONE CONSTANT airtime divisor, so it
cancels out of any ratio. The `(long)` columns let a short-CP arm bank a spurious
1260/1104 = +14.13% against a comparator DENIED the same lever; the `(joint)` columns are
the honest ones (D4). A `(joint)` miss is NOT evidence against short CP -- the bar is
structurally blind to an airtime lever.
COP-152 seed=0 arm-B bits=324096 data_air=176.55524999999974511 switch_air=7.60975000000000001
COP-152 segment seed=0 segment=2 bits=37440 data_air=37.46699999999998454 switch_air=-0.00000000000000000
COP-152 segment seed=0 segment=3 bits=30704 data_air=54.00399999999999068 switch_air=-0.00000000000000000
COP-152 seed=1 arm-B bits=341520 data_air=163.97425000000015416 switch_air=7.60975000000000001
COP-152 segment seed=1 segment=2 bits=40696 data_air=43.07899999999997931 switch_air=-0.00000000000000000
COP-152 segment seed=1 segment=3 bits=41848 data_air=35.92599999999997351 switch_air=-0.00000000000000000
COP-152 seed=2 arm-B bits=323128 data_air=190.79225000000002410 switch_air=7.60975000000000001
COP-152 segment seed=2 segment=2 bits=30080 data_air=48.48399999999996624 switch_air=-0.00000000000000000
COP-152 segment seed=2 segment=3 bits=30280 data_air=57.10899999999993781 switch_air=-0.00000000000000000
COP-152 seed=3 arm-B bits=334648 data_air=173.22025000000013506 switch_air=7.60975000000000001
COP-152 segment seed=3 segment=2 bits=41432 data_air=40.66399999999998727 switch_air=-0.00000000000000000
COP-152 segment seed=3 segment=3 bits=36464 data_air=47.58699999999996066 switch_air=-0.00000000000000000
COP-152 seed=4 arm-B bits=329680 data_air=178.02724999999986721 switch_air=7.60975000000000001
COP-152 segment seed=4 segment=2 bits=37888 data_air=45.35599999999998033 switch_air=-0.00000000000000000
COP-152 segment seed=4 segment=3 bits=39008 data_air=47.58699999999996066 switch_air=-0.00000000000000000

=== LEGACY CONTROL (airtime-blind bits-per-slot; proves the metric swap changed accounting only) ===
These are the PRE-COP-2 quantities, recomputed. On the `robust` base at seed 0 they must
reproduce the record: adaptive 326680 / best fixed (L4) 357424 / oracle 428936 /
adaptive-oracle 0.762 / adaptive-best-fixed 0.914.
COP-152 legacy seed=0 adaptive=330144 fixed=358848 oracle=429256 bars=Some((false, false)) (None = non-decisional)
seed 0   adaptive  330144 bits | best fixed (L4 )  358848 bits | oracle  429256 bits | a/o = 0.769  a/bf = 0.920
COP-152 legacy seed=1 adaptive=325680 fixed=350304 oracle=427056 bars=None (None = non-decisional)
seed 1   adaptive  325680 bits | best fixed (L4 )  350304 bits | oracle  427056 bits | a/o = 0.763  a/bf = 0.930
COP-152 legacy seed=2 adaptive=318512 fixed=367392 oracle=429416 bars=None (None = non-decisional)
seed 2   adaptive  318512 bits | best fixed (L4 )  367392 bits | oracle  429416 bits | a/o = 0.742  a/bf = 0.867
COP-152 legacy seed=3 adaptive=325208 fixed=350304 oracle=427552 bars=None (None = non-decisional)
seed 3   adaptive  325208 bits | best fixed (L4 )  350304 bits | oracle  427552 bits | a/o = 0.761  a/bf = 0.928
COP-152 legacy seed=4 adaptive=323888 fixed=364544 oracle=430880 bars=None (None = non-decisional)
seed 4   adaptive  323888 bits | best fixed (L4 )  364544 bits | oracle  430880 bits | a/o = 0.752  a/bf = 0.888
NOTE: `best fixed` here is an argmax over BITS (the legacy quantity). The primary table's
best-fixed is an argmax over GOODPUT, and the two genuinely select different levels -- that
is the metric change working, not a regression.

=== SWITCH ACCOUNTING (arm B) ===
Charged per switch: 3 frames under the old profile (Propose, Confirm, bare ack) + 2 under
the new one (CpSwitched, its ack) = the 5 droppable on-air frames cp_negotiator counts.
seed    switches   frames   switch_air %of arm B       gp x0       gp x1       gp x6
0              1        5         7.61     4.31%      1835.7      1759.8      1458.5
         switch LongCp -> ShortCp decided@3 effective@9 L1
1              1        5         7.61     4.64%      2082.8      1990.4      1629.1
         switch LongCp -> ShortCp decided@3 effective@9 L1
2              1        5         7.61     3.99%      1693.6      1628.7      1366.6
         switch LongCp -> ShortCp decided@3 effective@9 L1
3              1        5         7.61     4.39%      1931.9      1850.6      1528.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
4              1        5         7.61     4.27%      1851.9      1775.9      1473.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
x0 = no-charge upper bound, x1 = loss-free nominal, x6 = ARQ-budget ceiling
(DEFAULT_MAX_RETRANSMIT = 5). Not charged at all, all biasing arm B optimistic:
leg retransmissions, 4 half-duplex turnarounds at DEFAULT_TURNAROUND = 150 ms, and
handshakes that fail and revert via G1-G4 having spent the full budget for zero CP change.

=== CpGate DELAY-SPREAD HISTOGRAM (arm B, per schedule segment) ===
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up        97       0       0       0       0       0       0       0         3         0
s0 awgn ramp down      88       0       0       0       0       0       0       0        12         0
s0 watterson good      20      12       0       0       0       0       0       0        18         0
s0 watterson poor      24       0       0       0       0      14       0       0        12         0
s1 awgn ramp up        95       0       0       0       0       0       0       0         5         0
s1 awgn ramp down      91       0       0       0       0       0       0       0         9         0
s1 watterson good      27       9       0       0       0       0       0       0        14         0
s1 watterson poor      21       1       0       0       1      13       0       0        14         0
s2 awgn ramp up        97       0       0       0       0       0       0       0         3         0
s2 awgn ramp down      92       0       0       0       0       0       0       0         8         0
s2 watterson good      24       9       0       0       0       0       0       0        17         0
s2 watterson poor      19       0       0       0       0      17       0       0        14         0
s3 awgn ramp up        95       0       0       0       0       0       0       0         5         0
s3 awgn ramp down      90       0       0       0       0       0       0       0        10         0
s3 watterson good      32       6       0       0       0       0       0       0        12         0
s3 watterson poor      23       0       0       0       0      17       0       0        10         0
s4 awgn ramp up        93       0       0       0       0       0       0       0         7         0
s4 awgn ramp down      90       0       0       0       0       0       0       0        10         0
s4 watterson good      29       8       0       0       0       0       0       0        13         0
s4 watterson poor      21       0       0       0       0      17       0       0        12         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)
COP-152 standard arm-B-x1 operands=[[1759.8132109792873, 2240.662525879909, 3258.1447162773043], [1990.3953748601252, 2230.8350586611377, 3218.2865690479243], [1628.652936966361, 2329.109730848853, 3303.837074380825], [1850.6221312835246, 2299.6273291925386, 3259.7881633716934], [1775.9390638719665, 2221.007591442366, 3247.7859064410522]] bars=None (None = non-decisional)

=== TICKET COP-2 DELTA ===
Both answers below were PRE-COMMITTED in the plan (D4a) before this run.

PRIMARY -- the denominator delta (where a uniform airtime saving actually appears):
  best-fixed:  long-CP-only    1948.3 bps  ->  joint    2264.2 bps   (+16.21%)
  oracle    :  long-CP-only    2738.9 bps  ->  joint    3257.6 bps   (+18.94%)
  (the uniform CP factor is 1260/1104 = +14.130%; a value at or near it means the lever is
working exactly as arithmetic predicts, NOT that the bar moved)

SECONDARY -- does CP *adaptivity* add anything over fixed short CP?
  arm B (CP-adaptive)    1879.2 bps   vs   arm C (adaptive rate, fixed short CP)    1894.6 bps  (-0.82%)   [x0]
  arm B (CP-adaptive)    1801.1 bps   vs   arm C (adaptive rate, fixed short CP)    1894.6 bps  (-4.94%)   [x1]
  arm B / best-fixed(joint) = 0.830 (x0) / 0.795 (x1)  (bar: > 1.0)
  arm B / oracle(joint)     = 0.577 (x0) / 0.553 (x1)  (bar: >= 0.8)
  x0 = arm B's data airtime only; x1 = its handshake air charged once per switch. Arm C
has no handshake air at all, so only arm B's side of each comparison moves.
  upper bound (NOT a CP-isolation control): best hindsight-picked FIXED short-CP cell
     2264.2 bps -- fixed rate AND fixed CP, so B/this delta still conflates both adaptive
  dimensions; kept for reference only, per-frame in the seed table above as row "C".

HEADER-CODEWORD ISOLATION (arm C vs arm C', same short CP, header bandwidth ID only):
  arm C (header id 5)    1894.6 bps   vs   arm C' (header id 3)    1922.0 bps  (+1.45%)
  for scale: B-vs-C was -0.82% (x0). If |C-vs-C'| is comparable to |B-vs-C|, the
CP-specific conclusion above is confounded by the header codeword and should not be
drawn without controlling for it.

D9 DECISION RULE -- per-seed paired deltas, decided on SIGN CONSISTENCY:
Deltas are shown at x0 and x1. The sign counts below are tallied at BOTH, because a sign
that survives x0 but not x1 is a result the no-charge accounting manufactured.
seed         B-A (bps) x0/x1       B-C (bps) x0/x1       B/bf(joint)
0              +350.8/+274.9           -15.0/-90.9       0.819/0.785
1              +567.0/+474.6          -18.4/-110.7       0.934/0.892
2              +174.0/+109.1           -12.8/-77.8       0.727/0.699
3              +395.8/+314.5           -16.1/-97.4       0.840/0.805
4              +343.7/+267.7           -15.0/-90.9       0.834/0.800
  x0: B > A on 5/5 seeds; B > C on 0/5 seeds.
  x1: B > A on 5/5 seeds; B > C on 0/5 seeds.
  Sign-consistent (all 5 seeds one way) => a DIRECTIONAL result. Mixed signs => no result,
and the write-up must say the delta is inside this bench's own run-to-run band rather than
reporting the mean as if it were an effect. Pairing is exact only at the CHANNEL level: once
delivery outcomes diverge, RateLoop's trajectory diverges too, so the arms do not transmit
the same levels on the same frames.

REBUILD PLACEBO -- arm A 1512.9 bps vs arm P 1512.9 bps (+0.00%)
  If these differ by more than this bench's run-to-run band, arm B must be read against P,
not A, and the write-up must say so. Do NOT assume the transceiver rebuild is inert.

D1a SHORT-CP AIRTIME SHARE (arm B, on the control arm's pre-reduction basis):
  seed 0   levels<=4  45.29%   levels>=5  48.92%   (total short-CP  94.21%)
  seed 1   levels<=4  41.83%   levels>=5  51.93%   (total short-CP  93.76%)
  seed 2   levels<=4  53.24%   levels>=5  41.41%   (total short-CP  94.64%)
  seed 3   levels<=4  46.91%   levels>=5  47.18%   (total short-CP  94.10%)
  seed 4   levels<=4  47.21%   levels>=5  47.04%   (total short-CP  94.26%)
  Only the levels<=4 share is production-reachable: `select_ofdm_profile` routes every level
>= 5 to `vhf_wide()` and ignores cp_mode there, so the levels>=5 share is an explicit
UPPER-BOUND OVERSHOOT this bench collects and production cannot (D1a).

TRANSITION-COUNT GATE (pre-committed): if arm B yields <= 1 CpGate transition, this
schedule does not exercise CP *adaptivity* at all and the only content is the arm B vs
arm C fixed contrast -- which short_cp_fading_gate already covers. Do not describe a
latched single switch as adaptive control.
  transitions: 5 across 5 seed(s) (mean 1.0/run)

 frame   snr(dB)  level   (arm B, seed 0; level tracks the channel)
     0      3.0     L1
    15      7.1     L5
    30     11.1     L8
    45     15.1     L8
    60     19.2     L9
    75     23.2     L9
    90     27.3     L9
   105     28.6     L8
   120     24.6     L9
   135     20.5     L9
   150     16.5     L9
   165     12.5     L8
   180      8.4     L8
   195      4.4     L8
   210     24.0     L6
   225     24.0     L2
   240     24.0     L4
   255     24.0     L2
   270     24.0     L1
   285     24.0     L3

```
</details>

<details>
<summary>Complete baseline standard output</summary>

```text
    Blocking waiting for file lock on build directory
    Finished `release` profile [optimized] target(s) in 15.73s
     Running `/tmp/cop152-baseline/target/release/examples/closed_loop_arq standard`
=== Closed-loop adaptive rate + CP contrast (COP-2) ===
base pair    : hf_standard <-> hf_standard_short_cp (production-faithful, D1)
frames/run   : 300   seeds: [0, 1, 2, 3, 4]   runs: 120

  seed 3 fixed LongCp L1: 292/300 delivered, 319.5 bps
  seed 1 fixed LongCp L1: 296/300 delivered, 323.8 bps
  seed 0 fixed LongCp L1: 296/300 delivered, 323.8 bps
  seed 2 fixed LongCp L1: 293/300 delivered, 320.5 bps
  seed 4 fixed LongCp L1: 293/300 delivered, 320.5 bps
  seed 1 fixed LongCp L2: 290/300 delivered, 662.9 bps
  seed 2 fixed LongCp L2: 285/300 delivered, 651.4 bps
  seed 0 fixed LongCp L2: 282/300 delivered, 644.6 bps
  seed 3 fixed LongCp L2: 281/300 delivered, 642.3 bps
  seed 4 fixed LongCp L2: 291/300 delivered, 665.1 bps
  seed 0 fixed LongCp L3: 283/300 delivered, 1121.2 bps
  seed 2 fixed LongCp L3: 277/300 delivered, 1097.4 bps
  seed 1 fixed LongCp L3: 281/300 delivered, 1113.3 bps
  seed 3 fixed LongCp L3: 283/300 delivered, 1121.2 bps
  seed 4 fixed LongCp L3: 280/300 delivered, 1109.3 bps
  seed 0 fixed LongCp L4: 261/300 delivered, 1573.2 bps
  seed 2 fixed LongCp L4: 256/300 delivered, 1543.0 bps
  seed 1 fixed LongCp L4: 260/300 delivered, 1567.2 bps
  seed 4 fixed LongCp L4: 260/300 delivered, 1567.2 bps
  seed 3 fixed LongCp L4: 265/300 delivered, 1597.3 bps
  seed 0 fixed LongCp L5: 234/300 delivered, 1707.2 bps
  seed 2 fixed LongCp L5: 232/300 delivered, 1692.6 bps
  seed 1 fixed LongCp L5: 235/300 delivered, 1714.5 bps
  seed 4 fixed LongCp L5: 236/300 delivered, 1721.8 bps
  seed 3 fixed LongCp L5: 238/300 delivered, 1736.4 bps
  seed 0 fixed LongCp L6: 241/300 delivered, 1507.6 bps
  seed 2 fixed LongCp L6: 244/300 delivered, 1526.4 bps
  seed 1 fixed LongCp L6: 246/300 delivered, 1538.9 bps
  seed 4 fixed LongCp L6: 249/300 delivered, 1557.7 bps
  seed 3 fixed LongCp L6: 249/300 delivered, 1557.7 bps
  seed 0 fixed LongCp L7: 216/300 delivered, 2055.7 bps
  seed 2 fixed LongCp L7: 232/300 delivered, 2208.0 bps
  seed 1 fixed LongCp L7: 227/300 delivered, 2160.4 bps
  seed 3 fixed LongCp L7: 231/300 delivered, 2198.5 bps
  seed 4 fixed LongCp L7: 228/300 delivered, 2169.9 bps
  seed 2 fixed LongCp L8: 198/300 delivered, 2118.7 bps
  seed 0 fixed LongCp L8: 201/300 delivered, 2150.8 bps
  seed 1 fixed LongCp L8: 201/300 delivered, 2150.8 bps
  seed 3 fixed LongCp L8: 203/300 delivered, 2172.2 bps
  seed 4 fixed LongCp L8: 200/300 delivered, 2140.1 bps
  seed 2 fixed LongCp L9: 160/300 delivered, 2145.5 bps
  seed 0 fixed LongCp L9: 161/300 delivered, 2158.9 bps
  seed 3 fixed LongCp L9: 161/300 delivered, 2158.9 bps
  seed 1 fixed LongCp L9: 158/300 delivered, 2118.7 bps
  seed 4 fixed LongCp L9: 159/300 delivered, 2132.1 bps
  seed 0 fixed ShortCp L1: 290/300 delivered, 362.1 bps
  seed 2 fixed ShortCp L1: 290/300 delivered, 362.1 bps
  seed 3 fixed ShortCp L1: 292/300 delivered, 364.6 bps
  seed 1 fixed ShortCp L1: 296/300 delivered, 369.6 bps
  seed 4 fixed ShortCp L1: 286/300 delivered, 357.1 bps
  seed 0 fixed ShortCp L2: 286/300 delivered, 746.1 bps
  seed 2 fixed ShortCp L2: 285/300 delivered, 743.5 bps
  seed 3 fixed ShortCp L2: 290/300 delivered, 756.5 bps
  seed 1 fixed ShortCp L2: 289/300 delivered, 753.9 bps
  seed 4 fixed ShortCp L2: 282/300 delivered, 735.7 bps
  seed 0 fixed ShortCp L3: 278/300 delivered, 1257.0 bps
  seed 2 fixed ShortCp L3: 281/300 delivered, 1270.6 bps
  seed 3 fixed ShortCp L3: 286/300 delivered, 1293.2 bps
  seed 1 fixed ShortCp L3: 283/300 delivered, 1279.7 bps
  seed 4 fixed ShortCp L3: 292/300 delivered, 1320.3 bps
  seed 0 fixed ShortCp L4: 266/300 delivered, 1829.9 bps
  seed 2 fixed ShortCp L4: 265/300 delivered, 1823.0 bps
  seed 3 fixed ShortCp L4: 266/300 delivered, 1829.9 bps
  seed 1 fixed ShortCp L4: 269/300 delivered, 1850.5 bps
  seed 4 fixed ShortCp L4: 270/300 delivered, 1857.4 bps
  seed 0 fixed ShortCp L5: 247/300 delivered, 2056.7 bps
  seed 2 fixed ShortCp L5: 244/300 delivered, 2031.7 bps
  seed 1 fixed ShortCp L5: 247/300 delivered, 2056.7 bps
  seed 3 fixed ShortCp L5: 248/300 delivered, 2065.0 bps
  seed 4 fixed ShortCp L5: 247/300 delivered, 2056.7 bps
  seed 0 fixed ShortCp L6: 252/300 delivered, 1799.2 bps
  seed 2 fixed ShortCp L6: 249/300 delivered, 1777.8 bps
  seed 3 fixed ShortCp L6: 263/300 delivered, 1877.7 bps
  seed 1 fixed ShortCp L6: 255/300 delivered, 1820.6 bps
  seed 4 fixed ShortCp L6: 262/300 delivered, 1870.6 bps
  seed 0 fixed ShortCp L7: 220/300 delivered, 2389.6 bps
  seed 3 fixed ShortCp L7: 236/300 delivered, 2563.4 bps
  seed 2 fixed ShortCp L7: 230/300 delivered, 2498.2 bps
  seed 1 fixed ShortCp L7: 232/300 delivered, 2520.0 bps
  seed 4 fixed ShortCp L7: 236/300 delivered, 2563.4 bps
  seed 0 fixed ShortCp L8: 204/300 delivered, 2491.4 bps
  seed 1 fixed ShortCp L8: 203/300 delivered, 2479.1 bps
  seed 3 fixed ShortCp L8: 203/300 delivered, 2479.1 bps
  seed 2 fixed ShortCp L8: 196/300 delivered, 2393.7 bps
  seed 4 fixed ShortCp L8: 207/300 delivered, 2528.0 bps
  seed 0 fixed ShortCp L9: 160/300 delivered, 2448.7 bps
  seed 3 fixed ShortCp L9: 159/300 delivered, 2433.4 bps
  seed 1 fixed ShortCp L9: 160/300 delivered, 2448.7 bps
  seed 2 fixed ShortCp L9: 158/300 delivered, 2418.1 bps
  seed 4 fixed ShortCp L9: 161/300 delivered, 2464.0 bps
  seed 0 arm A  : 1741.0 bps
  seed 3 arm A  : 1854.2 bps
  seed 1 arm A  : 1938.5 bps
  seed 2 arm A  : 1862.0 bps
  seed 4 arm A  : 1813.3 bps
  seed 0 arm C  : 2257.7 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 3 arm C  : 2449.6 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 1 arm C  : 2265.7 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 2 arm C  : 2238.2 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 4 arm C  : 2632.8 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 0 arm C' : 2258.9 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 3 arm C' : 2447.4 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 1 arm C' : 2261.6 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 2 arm C' : 2240.3 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 4 arm C' : 2578.2 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 0 arm B  : 2239.8 bps, 1 switches
  seed 3 arm B  : 2429.0 bps, 1 switches
  seed 1 arm B  : 2247.9 bps, 1 switches
  seed 2 arm B  : 2220.4 bps, 1 switches
  seed 4 arm B  : 2609.5 bps, 1 switches
  seed 0 arm B0 : 2247.9 bps, 1 switches
  seed 3 arm B0 : 2438.2 bps, 1 switches
  seed 4 arm B0 : 2620.0 bps, 1 switches
  seed 2 arm B0 : 2228.4 bps, 1 switches
  seed 1 arm B0 : 2255.9 bps, 1 switches
  seed 0 arm P  : 1741.0 bps (1 rebuilds; arm B had 1)
  seed 3 arm P  : 1854.2 bps (1 rebuilds; arm B had 1)
  seed 4 arm P  : 1813.3 bps (1 rebuilds; arm B had 1)
  seed 2 arm P  : 1862.0 bps (1 rebuilds; arm B had 1)
  seed 1 arm P  : 1938.5 bps (1 rebuilds; arm B had 1)

=== PRIMARY: airtime-normalized goodput (bits/s) ===
The `airtime(s)` and `goodput` columns are DATA-AIRTIME-ONLY (= switch cost x0) for EVERY
arm, arm B included. Arm B's CP-handshake air is real and is charged separately in the
SWITCH ACCOUNTING block below (and as the x1 half of every ratio in THE BAR); it is NOT in
this table's denominator. Arms A / P / B0 / C / C' and all 18 comparator cells have no
handshake air at all, so for them x0 is the whole story.
seed   arm       deliv       bits  airtime(s)    goodput     FER   delivery 95% CI
0      A      276/300      340968      195.85     1741.0    8.0%   0.884-0.946
0      P      276/300      340968      195.85     1741.0    8.0%   0.884-0.946
0      B      272/300      350192      156.35     2239.8    9.3%   0.868-0.935
0      B0     272/300      350192      155.79     2247.9    9.3%   0.868-0.935
0      C      272/300      350192      155.11     2257.7    9.3%   0.868-0.935
0      C'     272/300      351632      155.66     2258.9    9.3%   0.868-0.935
0      C(fix)  204/300      257856      103.50     2491.4   32.0%   0.625-0.730    (L8)
1      A      268/300      349184      180.13     1938.5   10.7%   0.853-0.923
1      P      268/300      349184      180.13     1938.5   10.7%   0.853-0.923
1      B      275/300      353472      157.24     2247.9    8.3%   0.880-0.943
1      B0     275/300      353472      156.68     2255.9    8.3%   0.880-0.943
1      C      275/300      353472      156.01     2265.7    8.3%   0.880-0.943
1      C'     275/300      352832      156.01     2261.6    8.3%   0.880-0.943
1      C(fix)  232/300      330368      131.10     2520.0   22.7%   0.723-0.817    (L7)
2      A      267/300      344040      184.77     1862.0   11.0%   0.850-0.921
2      P      267/300      344040      184.77     1862.0   11.0%   0.850-0.921
2      B      268/300      344648      155.22     2220.4   10.7%   0.853-0.923
2      B0     268/300      344648      154.66     2228.4   10.7%   0.853-0.923
2      C      268/300      344648      153.98     2238.2   10.7%   0.853-0.923
2      C'     268/300      344968      153.98     2240.3   10.7%   0.853-0.923
2      C(fix)  230/300      327520      131.10     2498.2   23.3%   0.716-0.811    (L7)
3      A      271/300      348440      187.92     1854.2    9.7%   0.865-0.932
3      P      271/300      348440      187.92     1854.2    9.7%   0.865-0.932
3      B      271/300      356576      146.80     2429.0    9.7%   0.865-0.932
3      B0     271/300      356576      146.24     2438.2    9.7%   0.865-0.932
3      C      271/300      356576      145.57     2449.6    9.7%   0.865-0.932
3      C'     271/300      356256      145.57     2447.4    9.7%   0.865-0.932
3      C(fix)  236/300      336064      131.10     2563.4   21.3%   0.737-0.829    (L7)
4      A      274/300      346376      191.02     1813.3    8.7%   0.876-0.940
4      P      274/300      346376      191.02     1813.3    8.7%   0.876-0.940
4      B      273/300      363328      139.23     2609.5    9.0%   0.872-0.937
4      B0     273/300      363328      138.68     2620.0    9.0%   0.872-0.937
4      C      273/300      363328      138.00     2632.8    9.0%   0.872-0.937
4      C'     274/300      364752      141.47     2578.2    8.7%   0.876-0.940
4      C(fix)  236/300      336064      131.10     2563.4   21.3%   0.737-0.829    (L7)

=== COMPARATORS: 9-cell long-CP-only vs 18-cell joint level x CP (D4) ===
seed    bestfix(long)    lvl bestfix(joint)   cell  oracle(long) oracle(joint)
0              2158.9     L9         2491.4 shortL8        3040.7        3618.2
1              2160.4     L7         2520.0 shortL7        3055.6        3658.3
2              2208.0     L7         2498.2 shortL7        3072.8        3662.8
3              2198.5     L7         2563.4 shortL7        3087.1        3665.8
4              2169.9     L7         2563.4 shortL7        3081.6        3704.1

=== THE BAR (adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8) ===
Each ratio is printed at BOTH switch-cost multipliers: x0 (arm B's data airtime only) and
x1 (the loss-free nominal, one handshake charged per switch). x0 alone is what these rows
used to report, unlabelled -- an arm B that paid nothing for its own switches.
seed          B/bf(long)       B/orc(long)       B/bf(joint)      B/orc(joint)
0            1.037/0.996       0.737/0.707       0.899/0.863       0.619/0.594
1            1.041/0.999       0.736/0.707       0.892/0.857       0.614/0.590
2            1.006/0.965       0.723/0.694       0.889/0.853       0.606/0.582
3            1.105/1.058       0.787/0.754       0.948/0.907       0.663/0.635
4            1.203/1.149       0.847/0.809       1.018/0.973       0.704/0.673
(each cell is x0/x1)

REMINDER (Key Discovery 1): a uniform CP change is ONE CONSTANT airtime divisor, so it
cancels out of any ratio. The `(long)` columns let a short-CP arm bank a spurious
1260/1104 = +14.13% against a comparator DENIED the same lever; the `(joint)` columns are
the honest ones (D4). A `(joint)` miss is NOT evidence against short CP -- the bar is
structurally blind to an airtime lever.
COP-152 seed=0 arm-B bits=350192 data_air=156.34699999999961051 switch_air=6.48700000000000010
COP-152 segment seed=0 segment=2 bits=38096 data_air=34.47700000000001097 switch_air=-0.00000000000000000
COP-152 segment seed=0 segment=3 bits=34112 data_air=43.69999999999999574 switch_air=-0.00000000000000000
COP-152 seed=1 arm-B bits=353472 data_air=157.24399999999960187 switch_air=6.48700000000000010
COP-152 segment seed=1 segment=2 bits=39840 data_air=32.75200000000001666 switch_air=-0.00000000000000000
COP-152 segment seed=1 segment=3 bits=33904 data_air=46.41399999999999437 switch_air=-0.00000000000000000
COP-152 seed=2 arm-B bits=344648 data_air=155.21999999999982833 switch_air=6.48700000000000010
COP-152 segment seed=2 segment=2 bits=28128 data_air=41.79099999999999682 switch_air=-0.00000000000000000
COP-152 segment seed=2 segment=3 bits=36472 data_air=34.98300000000002541 switch_air=-0.00000000000000000
COP-152 seed=3 arm-B bits=356576 data_air=146.80199999999962301 switch_air=6.48700000000000010
COP-152 segment seed=3 segment=2 bits=39560 data_air=34.73000000000001108 switch_air=-0.00000000000000000
COP-152 segment seed=3 segment=3 bits=37288 data_air=33.99400000000001398 switch_air=-0.00000000000000000
COP-152 seed=4 arm-B bits=363328 data_air=139.23499999999978627 switch_air=6.48700000000000010
COP-152 segment seed=4 segment=2 bits=41224 data_air=30.36000000000002075 switch_air=-0.00000000000000000
COP-152 segment seed=4 segment=3 bits=42696 data_air=30.79700000000002547 switch_air=-0.00000000000000000

=== LEGACY CONTROL (airtime-blind bits-per-slot; proves the metric swap changed accounting only) ===
These are the PRE-COP-2 quantities, recomputed. On the `robust` base at seed 0 they must
reproduce the record: adaptive 326680 / best fixed (L4) 357424 / oracle 428936 /
adaptive-oracle 0.762 / adaptive-best-fixed 0.914.
COP-152 legacy seed=0 adaptive=340968 fixed=371664 oracle=436712 bars=None (None = non-decisional)
seed 0   adaptive  340968 bits | best fixed (L4 )  371664 bits | oracle  436712 bits | a/o = 0.781  a/bf = 0.917
COP-152 legacy seed=1 adaptive=349184 fixed=370240 oracle=436104 bars=None (None = non-decisional)
seed 1   adaptive  349184 bits | best fixed (L4 )  370240 bits | oracle  436104 bits | a/o = 0.801  a/bf = 0.943
COP-152 legacy seed=2 adaptive=344040 fixed=364544 oracle=437400 bars=None (None = non-decisional)
seed 2   adaptive  344040 bits | best fixed (L4 )  364544 bits | oracle  437400 bits | a/o = 0.787  a/bf = 0.944
COP-152 legacy seed=3 adaptive=348440 fixed=377360 oracle=438248 bars=None (None = non-decisional)
seed 3   adaptive  348440 bits | best fixed (L4 )  377360 bits | oracle  438248 bits | a/o = 0.795  a/bf = 0.923
COP-152 legacy seed=4 adaptive=346376 fixed=370240 oracle=439312 bars=None (None = non-decisional)
seed 4   adaptive  346376 bits | best fixed (L4 )  370240 bits | oracle  439312 bits | a/o = 0.788  a/bf = 0.936
NOTE: `best fixed` here is an argmax over BITS (the legacy quantity). The primary table's
best-fixed is an argmax over GOODPUT, and the two genuinely select different levels -- that
is the metric change working, not a regression.

=== SWITCH ACCOUNTING (arm B) ===
Charged per switch: 3 frames under the old profile (Propose, Confirm, bare ack) + 2 under
the new one (CpSwitched, its ack) = the 5 droppable on-air frames cp_negotiator counts.
seed    switches   frames   switch_air %of arm B       gp x0       gp x1       gp x6
0              1        5         6.49     4.15%      2239.8      2150.6      1793.4
         switch LongCp -> ShortCp decided@3 effective@9 L1
1              1        5         6.49     4.13%      2247.9      2158.9      1801.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
2              1        5         6.49     4.18%      2220.4      2131.3      1775.2
         switch LongCp -> ShortCp decided@3 effective@9 L1
3              1        5         6.49     4.42%      2429.0      2326.2      1919.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
4              1        5         6.49     4.66%      2609.5      2493.3      2039.4
         switch LongCp -> ShortCp decided@3 effective@9 L1
x0 = no-charge upper bound, x1 = loss-free nominal, x6 = ARQ-budget ceiling
(DEFAULT_MAX_RETRANSMIT = 5). Not charged at all, all biasing arm B optimistic:
leg retransmissions, 4 half-duplex turnarounds at DEFAULT_TURNAROUND = 150 ms, and
handshakes that fail and revert via G1-G4 having spent the full budget for zero CP change.

=== CpGate DELAY-SPREAD HISTOGRAM (arm B, per schedule segment) ===
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s0 awgn ramp down      98       0       0       0       0       0       0       0         2         0
s0 watterson good      25      11       0       0       0       0       0       0        14         0
s0 watterson poor      24       0       0       0       0      14       0       0        12         0
s1 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s1 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s1 watterson good      22      15       0       0       0       0       0       0        13         0
s1 watterson poor      20       0       0       0       0      19       0       0        11         0
s2 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s2 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s2 watterson good      28       5       0       0       0       0       0       0        17         0
s2 watterson poor      15       0       0       0       0      21       0       0        14         0
s3 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s3 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s3 watterson good      32       4       0       0       0       0       0       0        14         0
s3 watterson poor      22       0       0       0       0      14       0       0        14         0
s4 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s4 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s4 watterson good      24      11       0       0       0       0       0       0        15         0
s4 watterson poor      17       0       0       0       0      22       0       0        11         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)
COP-152 standard arm-B-x1 operands=[[2150.6073670118085, 2491.3623188405863, 3618.2186707233354], [2158.8581270498617, 2519.9694889397483, 3658.3252238956748], [2131.311569690863, 2498.2456140350955, 3662.812316559057], [2326.168218202225, 2563.4172387490544, 3665.815170102382], [2493.295452985826, 2563.4172387490544, 3704.09838739494]] bars=Some((false, false)) (None = non-decisional)

=== TICKET COP-2 DELTA ===
Both answers below were PRE-COMMITTED in the plan (D4a) before this run.

PRIMARY -- the denominator delta (where a uniform airtime saving actually appears):
  best-fixed:  long-CP-only    2179.1 bps  ->  joint    2527.3 bps   (+15.98%)
  oracle    :  long-CP-only    3067.6 bps  ->  joint    3661.9 bps   (+19.37%)
  (the uniform CP factor is 1260/1104 = +14.130%; a value at or near it means the lever is
working exactly as arithmetic predicts, NOT that the bar moved)

SECONDARY -- does CP *adaptivity* add anything over fixed short CP?
  arm B (CP-adaptive)    2349.3 bps   vs   arm C (adaptive rate, fixed short CP)    2368.8 bps  (-0.82%)   [x0]
  arm B (CP-adaptive)    2252.0 bps   vs   arm C (adaptive rate, fixed short CP)    2368.8 bps  (-4.93%)   [x1]
  arm B / best-fixed(joint) = 0.930 (x0) / 0.891 (x1)  (bar: > 1.0)
  arm B / oracle(joint)     = 0.642 (x0) / 0.615 (x1)  (bar: >= 0.8)
  x0 = arm B's data airtime only; x1 = its handshake air charged once per switch. Arm C
has no handshake air at all, so only arm B's side of each comparison moves.
  upper bound (NOT a CP-isolation control): best hindsight-picked FIXED short-CP cell
     2527.3 bps -- fixed rate AND fixed CP, so B/this delta still conflates both adaptive
  dimensions; kept for reference only, per-frame in the seed table above as row "C".

HEADER-CODEWORD ISOLATION (arm C vs arm C', same short CP, header bandwidth ID only):
  arm C (header id 4)    2368.8 bps   vs   arm C' (header id 1)    2357.3 bps  (-0.49%)
  for scale: B-vs-C was -0.82% (x0). If |C-vs-C'| is comparable to |B-vs-C|, the
CP-specific conclusion above is confounded by the header codeword and should not be
drawn without controlling for it.

D9 DECISION RULE -- per-seed paired deltas, decided on SIGN CONSISTENCY:
Deltas are shown at x0 and x1. The sign counts below are tallied at BOTH, because a sign
that survives x0 but not x1 is a result the no-charge accounting manufactured.
seed         B-A (bps) x0/x1       B-C (bps) x0/x1       B/bf(joint)
0              +498.9/+409.7          -17.8/-107.1       0.899/0.863
1              +309.4/+220.3          -17.8/-106.9       0.892/0.857
2              +358.4/+269.4          -17.8/-106.9       0.889/0.853
3              +574.8/+472.0          -20.6/-123.4       0.948/0.907
4              +796.2/+680.0          -23.4/-139.5       1.018/0.973
  x0: B > A on 5/5 seeds; B > C on 0/5 seeds.
  x1: B > A on 5/5 seeds; B > C on 0/5 seeds.
  Sign-consistent (all 5 seeds one way) => a DIRECTIONAL result. Mixed signs => no result,
and the write-up must say the delta is inside this bench's own run-to-run band rather than
reporting the mean as if it were an effect. Pairing is exact only at the CHANNEL level: once
delivery outcomes diverge, RateLoop's trajectory diverges too, so the arms do not transmit
the same levels on the same frames.

REBUILD PLACEBO -- arm A 1841.8 bps vs arm P 1841.8 bps (+0.00%)
  If these differ by more than this bench's run-to-run band, arm B must be read against P,
not A, and the write-up must say so. Do NOT assume the transceiver rebuild is inert.

D1a SHORT-CP AIRTIME SHARE (arm B, on the control arm's pre-reduction basis):
  seed 0   levels<=4  45.97%   levels>=5  48.40%   (total short-CP  94.37%)
  seed 1   levels<=4  45.11%   levels>=5  49.28%   (total short-CP  94.40%)
  seed 2   levels<=4  41.79%   levels>=5  52.53%   (total short-CP  94.32%)
  seed 3   levels<=4  40.70%   levels>=5  53.29%   (total short-CP  94.00%)
  seed 4   levels<=4  33.37%   levels>=5  60.30%   (total short-CP  93.67%)
  Only the levels<=4 share is production-reachable: `select_ofdm_profile` routes every level
>= 5 to `vhf_wide()` and ignores cp_mode there, so the levels>=5 share is an explicit
UPPER-BOUND OVERSHOOT this bench collects and production cannot (D1a).

TRANSITION-COUNT GATE (pre-committed): if arm B yields <= 1 CpGate transition, this
schedule does not exercise CP *adaptivity* at all and the only content is the arm B vs
arm C fixed contrast -- which short_cp_fading_gate already covers. Do not describe a
latched single switch as adaptive control.
  transitions: 5 across 5 seed(s) (mean 1.0/run)

 frame   snr(dB)  level   (arm B, seed 0; level tracks the channel)
     0      3.0     L1
    15      7.1     L5
    30     11.1     L8
    45     15.1     L8
    60     19.2     L9
    75     23.2     L9
    90     27.3     L9
   105     28.6     L9
   120     24.6     L9
   135     20.5     L9
   150     16.5     L9
   165     12.5     L8
   180      8.4     L8
   195      4.4     L8
   210     24.0     L4
   225     24.0     L2
   240     24.0     L4
   255     24.0     L3
   270     24.0     L2
   285     24.0     L3

```
</details>

<details>
<summary>Complete candidate robust output</summary>

```text
   Compiling coppa-protocol v0.7.0 (/workspace/tmp/catalyst-runner-workspace-T8bUvX/crates/coppa-protocol)
   Compiling coppa-bench v0.7.0 (/workspace/tmp/catalyst-runner-workspace-T8bUvX/crates/coppa-bench)
    Finished `release` profile [optimized] target(s) in 20.97s
     Running `target/release/examples/closed_loop_arq`
=== Closed-loop adaptive rate + CP contrast (COP-2) ===
base pair    : hf_robust <-> synthetic robust short-CP (metric question, D1c)
frames/run   : 300   seeds: [0, 1, 2, 3, 4]   runs: 120
NOTE         : the short-CP half of this pair is a BENCH-LOCAL SYNTHETIC profile with an
unallocated bandwidth_id (5). `select_ofdm_profile` can never select it and the
engine can never negotiate it, so this base measures the CP lever's MAGNITUDE,
not a shippable feature. Use the `standard` base for the shippable claim.

  seed 2 fixed LongCp L1: 294/300 delivered, 274.2 bps
  seed 0 fixed LongCp L1: 295/300 delivered, 275.1 bps
  seed 3 fixed LongCp L1: 297/300 delivered, 277.0 bps
  seed 4 fixed LongCp L1: 293/300 delivered, 273.3 bps
  seed 1 fixed LongCp L1: 299/300 delivered, 278.8 bps
  seed 2 fixed LongCp L2: 285/300 delivered, 555.3 bps
  seed 3 fixed LongCp L2: 283/300 delivered, 551.4 bps
  seed 0 fixed LongCp L2: 283/300 delivered, 551.4 bps
  seed 1 fixed LongCp L2: 287/300 delivered, 559.2 bps
  seed 4 fixed LongCp L2: 282/300 delivered, 549.5 bps
  seed 2 fixed LongCp L3: 277/300 delivered, 968.3 bps
  seed 3 fixed LongCp L3: 271/300 delivered, 947.4 bps
  seed 1 fixed LongCp L3: 272/300 delivered, 950.9 bps
  seed 0 fixed LongCp L3: 281/300 delivered, 982.3 bps
  seed 4 fixed LongCp L3: 279/300 delivered, 975.3 bps
  seed 2 fixed LongCp L4: 258/300 delivered, 1372.1 bps
  seed 4 fixed LongCp L4: 256/300 delivered, 1361.5 bps
  seed 3 fixed LongCp L4: 246/300 delivered, 1308.3 bps
  seed 1 fixed LongCp L4: 246/300 delivered, 1308.3 bps
  seed 0 fixed LongCp L4: 252/300 delivered, 1340.2 bps
  seed 2 fixed LongCp L5: 235/300 delivered, 1508.8 bps
  seed 4 fixed LongCp L5: 245/300 delivered, 1573.0 bps
  seed 3 fixed LongCp L5: 244/300 delivered, 1566.6 bps
  seed 1 fixed LongCp L5: 236/300 delivered, 1515.2 bps
  seed 0 fixed LongCp L5: 240/300 delivered, 1540.9 bps
  seed 2 fixed LongCp L6: 245/300 delivered, 1386.7 bps
  seed 4 fixed LongCp L6: 250/300 delivered, 1415.0 bps
  seed 0 fixed LongCp L6: 238/300 delivered, 1347.0 bps
  seed 3 fixed LongCp L6: 250/300 delivered, 1415.0 bps
  seed 1 fixed LongCp L6: 245/300 delivered, 1386.7 bps
  seed 2 fixed LongCp L7: 222/300 delivered, 1911.6 bps
  seed 0 fixed LongCp L7: 226/300 delivered, 1946.0 bps
  seed 4 fixed LongCp L7: 226/300 delivered, 1946.0 bps
  seed 3 fixed LongCp L7: 229/300 delivered, 1971.9 bps
  seed 1 fixed LongCp L7: 224/300 delivered, 1928.8 bps
  seed 2 fixed LongCp L8: 189/300 delivered, 1896.0 bps
  seed 0 fixed LongCp L8: 193/300 delivered, 1936.1 bps
  seed 4 fixed LongCp L8: 192/300 delivered, 1926.1 bps
  seed 3 fixed LongCp L8: 194/300 delivered, 1946.2 bps
  seed 1 fixed LongCp L8: 196/300 delivered, 1966.2 bps
  seed 2 fixed LongCp L9: 122/300 delivered, 1533.7 bps
  seed 0 fixed LongCp L9: 122/300 delivered, 1533.7 bps
  seed 4 fixed LongCp L9: 121/300 delivered, 1521.1 bps
  seed 3 fixed LongCp L9: 122/300 delivered, 1533.7 bps
  seed 1 fixed LongCp L9: 126/300 delivered, 1584.0 bps
  seed 2 fixed ShortCp L1: 291/300 delivered, 309.7 bps
  seed 0 fixed ShortCp L1: 296/300 delivered, 315.1 bps
  seed 3 fixed ShortCp L1: 295/300 delivered, 314.0 bps
  seed 4 fixed ShortCp L1: 295/300 delivered, 314.0 bps
  seed 1 fixed ShortCp L1: 291/300 delivered, 309.7 bps
  seed 2 fixed ShortCp L2: 278/300 delivered, 618.2 bps
  seed 4 fixed ShortCp L2: 291/300 delivered, 647.1 bps
  seed 0 fixed ShortCp L2: 287/300 delivered, 638.2 bps
  seed 1 fixed ShortCp L2: 282/300 delivered, 627.1 bps
  seed 3 fixed ShortCp L2: 291/300 delivered, 647.1 bps
  seed 2 fixed ShortCp L3: 275/300 delivered, 1097.2 bps
  seed 0 fixed ShortCp L3: 278/300 delivered, 1109.2 bps
  seed 4 fixed ShortCp L3: 286/300 delivered, 1141.1 bps
  seed 1 fixed ShortCp L3: 282/300 delivered, 1125.1 bps
  seed 3 fixed ShortCp L3: 282/300 delivered, 1125.1 bps
  seed 2 fixed ShortCp L4: 255/300 delivered, 1547.8 bps
  seed 0 fixed ShortCp L4: 255/300 delivered, 1547.8 bps
  seed 4 fixed ShortCp L4: 271/300 delivered, 1644.9 bps
  seed 1 fixed ShortCp L4: 264/300 delivered, 1602.5 bps
  seed 3 fixed ShortCp L4: 267/300 delivered, 1620.7 bps
  seed 0 fixed ShortCp L5: 248/300 delivered, 1817.2 bps
  seed 2 fixed ShortCp L5: 255/300 delivered, 1868.5 bps
  seed 4 fixed ShortCp L5: 249/300 delivered, 1824.6 bps
  seed 1 fixed ShortCp L5: 250/300 delivered, 1831.9 bps
  seed 3 fixed ShortCp L5: 250/300 delivered, 1831.9 bps
  seed 0 fixed ShortCp L6: 250/300 delivered, 1614.9 bps
  seed 2 fixed ShortCp L6: 260/300 delivered, 1679.5 bps
  seed 4 fixed ShortCp L6: 254/300 delivered, 1640.7 bps
  seed 1 fixed ShortCp L6: 251/300 delivered, 1621.4 bps
  seed 3 fixed ShortCp L6: 255/300 delivered, 1647.2 bps
  seed 0 fixed ShortCp L7: 228/300 delivered, 2240.7 bps
  seed 2 fixed ShortCp L7: 237/300 delivered, 2329.1 bps
  seed 4 fixed ShortCp L7: 226/300 delivered, 2221.0 bps
  seed 1 fixed ShortCp L7: 227/300 delivered, 2230.8 bps
  seed 3 fixed ShortCp L7: 234/300 delivered, 2299.6 bps
  seed 0 fixed ShortCp L8: 192/300 delivered, 2198.3 bps
  seed 4 fixed ShortCp L8: 190/300 delivered, 2175.4 bps
  seed 2 fixed ShortCp L8: 196/300 delivered, 2244.1 bps
  seed 1 fixed ShortCp L8: 190/300 delivered, 2175.4 bps
  seed 3 fixed ShortCp L8: 196/300 delivered, 2244.1 bps
  seed 4 fixed ShortCp L9: 124/300 delivered, 1779.1 bps
  seed 0 fixed ShortCp L9: 119/300 delivered, 1707.4 bps
  seed 2 fixed ShortCp L9: 124/300 delivered, 1779.1 bps
  seed 1 fixed ShortCp L9: 123/300 delivered, 1764.8 bps
  seed 3 fixed ShortCp L9: 124/300 delivered, 1779.1 bps
  seed 4 arm A  : 1517.1 bps
  seed 0 arm A  : 1532.0 bps
  seed 2 arm A  : 1626.9 bps
  seed 1 arm A  : 1601.2 bps
  seed 3 arm A  : 1459.6 bps
  seed 4 arm C  : 1883.8 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 1 arm C  : 1971.2 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 0 arm C  : 1740.0 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 2 arm C  : 1683.9 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 3 arm C  : 1884.3 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 1 arm C' : 1971.2 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 4 arm C' : 1898.3 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 0 arm C' : 1740.0 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 2 arm C' : 1679.9 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 3 arm C' : 1884.3 bps (arm C, header bandwidth pinned to long-CP's ID 3 -- header-codeword isolation)
  seed 1 arm B  : 1954.1 bps, 1 switches
  seed 4 arm B  : 1868.4 bps, 1 switches
  seed 0 arm B  : 1726.3 bps, 1 switches
  seed 2 arm B  : 1671.0 bps, 1 switches
  seed 3 arm B  : 1868.6 bps, 1 switches
  seed 1 arm B0 : 1961.7 bps, 1 switches
  seed 4 arm B0 : 1875.2 bps, 1 switches
  seed 0 arm B0 : 1732.4 bps, 1 switches
  seed 3 arm B0 : 1875.6 bps, 1 switches
  seed 2 arm B0 : 1676.7 bps, 1 switches
  seed 1 arm P  : 1601.2 bps (1 rebuilds; arm B had 1)
  seed 4 arm P  : 1517.1 bps (1 rebuilds; arm B had 1)
  seed 0 arm P  : 1532.0 bps (1 rebuilds; arm B had 1)
  seed 3 arm P  : 1459.6 bps (1 rebuilds; arm B had 1)
  seed 2 arm P  : 1626.9 bps (1 rebuilds; arm B had 1)

=== PRIMARY: airtime-normalized goodput (bits/s) ===
The `airtime(s)` and `goodput` columns are DATA-AIRTIME-ONLY (= switch cost x0) for EVERY
arm, arm B included. Arm B's CP-handshake air is real and is charged separately in the
SWITCH ACCOUNTING block below (and as the x1 half of every ratio in THE BAR); it is NOT in
this table's denominator. Arms A / P / B0 / C / C' and all 18 comparator cells have no
handshake air at all, so for them x0 is the whole story.
seed   arm       deliv       bits  airtime(s)    goodput     FER   delivery 95% CI
0      A      245/300      314368      205.20     1532.0   18.3%   0.769-0.856
0      P      245/300      314368      205.20     1532.0   18.3%   0.769-0.856
0      B      248/300      313040      181.34     1726.3   17.3%   0.780-0.865
0      B0     248/300      313040      180.70     1732.4   17.3%   0.780-0.865
0      C      248/300      313040      179.91     1740.0   17.3%   0.780-0.865
0      C'     248/300      313040      179.91     1740.0   17.3%   0.780-0.865
0      C(fix)  228/300      324672      144.90     2240.7   24.0%   0.709-0.805    (L7)
1      A      241/300      315744      197.19     1601.2   19.7%   0.755-0.844
1      P      241/300      315744      197.19     1601.2   19.7%   0.755-0.844
1      B      246/300      322712      165.15     1954.1   18.0%   0.773-0.859
1      B0     246/300      322712      164.51     1961.7   18.0%   0.773-0.859
1      C      246/300      322712      163.71     1971.2   18.0%   0.773-0.859
1      C'     246/300      322712      163.71     1971.2   18.0%   0.773-0.859
1      C(fix)  227/300      323248      144.90     2230.8   24.3%   0.705-0.802    (L7)
2      A      239/300      314864      193.54     1626.9   20.3%   0.748-0.838
2      P      239/300      314864      193.54     1626.9   20.3%   0.748-0.838
2      B      246/300      312432      186.97     1671.0   18.0%   0.773-0.859
2      B0     246/300      312432      186.33     1676.7   18.0%   0.773-0.859
2      C      246/300      312432      185.54     1683.9   18.0%   0.773-0.859
2      C'     249/300      313776      186.78     1679.9   17.0%   0.783-0.868
2      C(fix)  237/300      337488      144.90     2329.1   21.0%   0.740-0.832    (L7)
3      A      248/300      314824      215.70     1459.6   17.3%   0.780-0.865
3      P      248/300      314824      215.70     1459.6   17.3%   0.780-0.865
3      B      248/300      321184      171.89     1868.6   17.3%   0.780-0.865
3      B0     248/300      321184      171.25     1875.6   17.3%   0.780-0.865
3      C      248/300      321184      170.45     1884.3   17.3%   0.780-0.865
3      C'     248/300      321184      170.45     1884.3   17.3%   0.780-0.865
3      C(fix)  234/300      333216      144.90     2299.6   22.0%   0.730-0.823    (L7)
4      A      240/300      310752      204.83     1517.1   20.0%   0.751-0.841
4      P      240/300      310752      204.83     1517.1   20.0%   0.751-0.841
4      B      248/300      327080      175.06     1868.4   17.3%   0.780-0.865
4      B0     248/300      327080      174.42     1875.2   17.3%   0.780-0.865
4      C      248/300      327080      173.63     1883.8   17.3%   0.780-0.865
4      C'     246/300      324880      171.14     1898.3   18.0%   0.773-0.859
4      C(fix)  226/300      321824      144.90     2221.0   24.7%   0.702-0.799    (L7)

=== COMPARATORS: 9-cell long-CP-only vs 18-cell joint level x CP (D4) ===
seed    bestfix(long)    lvl bestfix(joint)   cell  oracle(long) oracle(joint)
0              1946.0     L7         2240.7 shortL7        2732.1        3258.1
1              1966.2     L8         2230.8 shortL7        2753.7        3218.3
2              1911.6     L7         2329.1 shortL7        2708.8        3303.8
3              1971.9     L7         2299.6 shortL7        2749.7        3259.8
4              1946.0     L7         2221.0 shortL7        2750.0        3247.8

=== THE BAR (adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8) ===
Each ratio is printed at BOTH switch-cost multipliers: x0 (arm B's data airtime only) and
x1 (the loss-free nominal, one handshake charged per switch). x0 alone is what these rows
used to report, unlabelled -- an arm B that paid nothing for its own switches.
seed          B/bf(long)       B/orc(long)       B/bf(joint)      B/orc(joint)
0            0.887/0.851       0.632/0.606       0.770/0.739       0.530/0.508
1            0.994/0.950       0.710/0.678       0.876/0.837       0.607/0.580
2            0.874/0.840       0.617/0.593       0.717/0.689       0.506/0.486
3            0.948/0.907       0.680/0.651       0.813/0.778       0.573/0.549
4            0.960/0.920       0.679/0.651       0.841/0.806       0.575/0.551
(each cell is x0/x1)

REMINDER (Key Discovery 1): a uniform CP change is ONE CONSTANT airtime divisor, so it
cancels out of any ratio. The `(long)` columns let a short-CP arm bank a spurious
1260/1104 = +14.13% against a comparator DENIED the same lever; the `(joint)` columns are
the honest ones (D4). A `(joint)` miss is NOT evidence against short CP -- the bar is
structurally blind to an airtime lever.
COP-152 seed=0 arm-B bits=313040 data_air=181.33924999999970851 switch_air=7.60975000000000001
COP-152 segment seed=0 segment=2 bits=33088 data_air=37.58199999999997942 switch_air=-0.00000000000000000
COP-152 segment seed=0 segment=3 bits=30256 data_air=57.52299999999998192 switch_air=-0.00000000000000000
COP-152 seed=1 arm-B bits=322712 data_air=165.14724999999992860 switch_air=7.60975000000000001
COP-152 segment seed=1 segment=2 bits=39856 data_air=33.99399999999998556 switch_air=-0.00000000000000000
COP-152 segment seed=1 segment=3 bits=35528 data_air=45.95399999999996510 switch_air=-0.00000000000000000
COP-152 seed=2 arm-B bits=312432 data_air=186.97424999999992679 switch_air=7.60975000000000001
COP-152 segment seed=2 segment=2 bits=29144 data_air=45.17199999999996862 switch_air=-0.00000000000000000
COP-152 segment seed=2 segment=3 bits=30728 data_air=56.48799999999992849 switch_air=-0.00000000000000000
COP-152 seed=3 arm-B bits=321184 data_air=171.88624999999998977 switch_air=7.60975000000000001
COP-152 segment seed=3 segment=2 bits=39760 data_air=39.76699999999998170 switch_air=-0.00000000000000000
COP-152 segment seed=3 segment=3 bits=36016 data_air=47.37999999999996703 switch_air=-0.00000000000000000
COP-152 seed=4 arm-B bits=327080 data_air=175.06024999999968372 switch_air=7.60975000000000001
COP-152 segment seed=4 segment=2 bits=41344 data_air=42.25099999999997635 switch_air=-0.00000000000000000
COP-152 segment seed=4 segment=3 bits=41448 data_air=47.49499999999996191 switch_air=-0.00000000000000000

=== LEGACY CONTROL (airtime-blind bits-per-slot; proves the metric swap changed accounting only) ===
These are the PRE-COP-2 quantities, recomputed. On the `robust` base at seed 0 they must
reproduce the record: adaptive 326680 / best fixed (L4) 357424 / oracle 428936 /
adaptive-oracle 0.762 / adaptive-best-fixed 0.914.
COP-152 legacy seed=0 adaptive=314368 fixed=358848 oracle=429256 bars=Some((false, false)) (None = non-decisional)
seed 0   adaptive  314368 bits | best fixed (L4 )  358848 bits | oracle  429256 bits | a/o = 0.732  a/bf = 0.876
COP-152 legacy seed=1 adaptive=315744 fixed=350304 oracle=427056 bars=None (None = non-decisional)
seed 1   adaptive  315744 bits | best fixed (L4 )  350304 bits | oracle  427056 bits | a/o = 0.739  a/bf = 0.901
COP-152 legacy seed=2 adaptive=314864 fixed=367392 oracle=429416 bars=None (None = non-decisional)
seed 2   adaptive  314864 bits | best fixed (L4 )  367392 bits | oracle  429416 bits | a/o = 0.733  a/bf = 0.857
COP-152 legacy seed=3 adaptive=314824 fixed=350304 oracle=427552 bars=None (None = non-decisional)
seed 3   adaptive  314824 bits | best fixed (L4 )  350304 bits | oracle  427552 bits | a/o = 0.736  a/bf = 0.899
COP-152 legacy seed=4 adaptive=310752 fixed=364544 oracle=430880 bars=None (None = non-decisional)
seed 4   adaptive  310752 bits | best fixed (L4 )  364544 bits | oracle  430880 bits | a/o = 0.721  a/bf = 0.852
NOTE: `best fixed` here is an argmax over BITS (the legacy quantity). The primary table's
best-fixed is an argmax over GOODPUT, and the two genuinely select different levels -- that
is the metric change working, not a regression.

=== SWITCH ACCOUNTING (arm B) ===
Charged per switch: 3 frames under the old profile (Propose, Confirm, bare ack) + 2 under
the new one (CpSwitched, its ack) = the 5 droppable on-air frames cp_negotiator counts.
seed    switches   frames   switch_air %of arm B       gp x0       gp x1       gp x6
0              1        5         7.61     4.20%      1726.3      1656.7      1379.0
         switch LongCp -> ShortCp decided@3 effective@9 L1
1              1        5         7.61     4.61%      1954.1      1868.0      1530.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
2              1        5         7.61     4.07%      1671.0      1605.6      1343.0
         switch LongCp -> ShortCp decided@3 effective@9 L1
3              1        5         7.61     4.43%      1868.6      1789.4      1476.4
         switch LongCp -> ShortCp decided@3 effective@9 L1
4              1        5         7.61     4.35%      1868.4      1790.6      1481.9
         switch LongCp -> ShortCp decided@3 effective@9 L1
x0 = no-charge upper bound, x1 = loss-free nominal, x6 = ARQ-budget ceiling
(DEFAULT_MAX_RETRANSMIT = 5). Not charged at all, all biasing arm B optimistic:
leg retransmissions, 4 half-duplex turnarounds at DEFAULT_TURNAROUND = 150 ms, and
handshakes that fail and revert via G1-G4 having spent the full budget for zero CP change.

=== CpGate DELAY-SPREAD HISTOGRAM (arm B, per schedule segment) ===
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up        92       0       0       0       0       0       0       0         8         0
s0 awgn ramp down      84       0       0       0       0       0       0       0        16         0
s0 watterson good      20      11       0       0       0       0       0       0        19         0
s0 watterson poor      29       0       0       0       0      12       0       0         9         0
s1 awgn ramp up        91       0       0       0       0       0       0       0         9         0
s1 awgn ramp down      83       0       0       0       0       0       0       0        17         0
s1 watterson good      21      12       0       0       0       0       0       0        17         0
s1 watterson poor      26       0       0       0       0      13       0       0        11         0
s2 awgn ramp up        92       0       0       0       0       0       0       0         8         0
s2 awgn ramp down      85       0       0       0       0       0       0       0        15         0
s2 watterson good      24       8       0       0       0       0       0       0        18         0
s2 watterson poor      20       0       0       0       0      17       0       0        13         0
s3 awgn ramp up        91       0       0       0       0       0       0       0         9         0
s3 awgn ramp down      83       0       0       0       0       0       0       0        17         0
s3 watterson good      27       8       0       0       0       0       0       0        15         0
s3 watterson poor      22       0       0       0       0      17       0       0        11         0
s4 awgn ramp up        91       0       0       0       0       0       0       0         9         0
s4 awgn ramp down      83       0       0       0       0       0       0       0        17         0
s4 watterson good      27       9       0       0       0       0       0       0        14         0
s4 watterson poor      21       0       0       0       0      17       0       0        12         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)
COP-152 standard arm-B-x1 operands=[[1656.7433540267507, 2240.662525879909, 3258.1447162773043], [1868.0111370306277, 2230.8350586611377, 3218.2865690479243], [1605.6407515520295, 2329.109730848853, 3303.837074380825], [1789.3657797388244, 2299.6273291925386, 3259.7881633716934], [1790.5512673126434, 2221.007591442366, 3247.7859064410522]] bars=None (None = non-decisional)

=== TICKET COP-2 DELTA ===
Both answers below were PRE-COMMITTED in the plan (D4a) before this run.

PRIMARY -- the denominator delta (where a uniform airtime saving actually appears):
  best-fixed:  long-CP-only    1948.3 bps  ->  joint    2264.2 bps   (+16.21%)
  oracle    :  long-CP-only    2738.9 bps  ->  joint    3257.6 bps   (+18.94%)
  (the uniform CP factor is 1260/1104 = +14.130%; a value at or near it means the lever is
working exactly as arithmetic predicts, NOT that the bar moved)

SECONDARY -- does CP *adaptivity* add anything over fixed short CP?
  arm B (CP-adaptive)    1817.7 bps   vs   arm C (adaptive rate, fixed short CP)    1832.6 bps  (-0.82%)   [x0]
  arm B (CP-adaptive)    1742.1 bps   vs   arm C (adaptive rate, fixed short CP)    1832.6 bps  (-4.94%)   [x1]
  arm B / best-fixed(joint) = 0.803 (x0) / 0.769 (x1)  (bar: > 1.0)
  arm B / oracle(joint)     = 0.558 (x0) / 0.535 (x1)  (bar: >= 0.8)
  x0 = arm B's data airtime only; x1 = its handshake air charged once per switch. Arm C
has no handshake air at all, so only arm B's side of each comparison moves.
  upper bound (NOT a CP-isolation control): best hindsight-picked FIXED short-CP cell
     2264.2 bps -- fixed rate AND fixed CP, so B/this delta still conflates both adaptive
  dimensions; kept for reference only, per-frame in the seed table above as row "C".

HEADER-CODEWORD ISOLATION (arm C vs arm C', same short CP, header bandwidth ID only):
  arm C (header id 5)    1832.6 bps   vs   arm C' (header id 3)    1834.7 bps  (+0.11%)
  for scale: B-vs-C was -0.82% (x0). If |C-vs-C'| is comparable to |B-vs-C|, the
CP-specific conclusion above is confounded by the header codeword and should not be
drawn without controlling for it.

D9 DECISION RULE -- per-seed paired deltas, decided on SIGN CONSISTENCY:
Deltas are shown at x0 and x1. The sign counts below are tallied at BOTH, because a sign
that survives x0 but not x1 is a result the no-charge accounting manufactured.
seed         B-A (bps) x0/x1       B-C (bps) x0/x1       B/bf(joint)
0              +194.2/+124.7           -13.8/-83.3       0.770/0.739
1              +352.9/+266.8          -17.1/-103.2       0.876/0.837
2                +44.1/-21.2           -12.9/-78.3       0.717/0.689
3              +409.0/+329.8           -15.7/-94.9       0.813/0.778
4              +351.3/+273.4           -15.4/-93.3       0.841/0.806
  x0: B > A on 5/5 seeds; B > C on 0/5 seeds.
  x1: B > A on 4/5 seeds; B > C on 0/5 seeds.
  Sign-consistent (all 5 seeds one way) => a DIRECTIONAL result. Mixed signs => no result,
and the write-up must say the delta is inside this bench's own run-to-run band rather than
reporting the mean as if it were an effect. Pairing is exact only at the CHANNEL level: once
delivery outcomes diverge, RateLoop's trajectory diverges too, so the arms do not transmit
the same levels on the same frames.

REBUILD PLACEBO -- arm A 1547.4 bps vs arm P 1547.4 bps (+0.00%)
  If these differ by more than this bench's run-to-run band, arm B must be read against P,
not A, and the write-up must say so. Do NOT assume the transceiver rebuild is inert.

D1a SHORT-CP AIRTIME SHARE (arm B, on the control arm's pre-reduction basis):
  seed 0   levels<=4  46.68%   levels>=5  47.69%   (total short-CP  94.36%)
  seed 1   levels<=4  38.93%   levels>=5  54.87%   (total short-CP  93.80%)
  seed 2   levels<=4  51.90%   levels>=5  42.63%   (total short-CP  94.53%)
  seed 3   levels<=4  48.21%   levels>=5  45.84%   (total short-CP  94.05%)
  seed 4   levels<=4  45.24%   levels>=5  48.92%   (total short-CP  94.16%)
  Only the levels<=4 share is production-reachable: `select_ofdm_profile` routes every level
>= 5 to `vhf_wide()` and ignores cp_mode there, so the levels>=5 share is an explicit
UPPER-BOUND OVERSHOOT this bench collects and production cannot (D1a).

TRANSITION-COUNT GATE (pre-committed): if arm B yields <= 1 CpGate transition, this
schedule does not exercise CP *adaptivity* at all and the only content is the arm B vs
arm C fixed contrast -- which short_cp_fading_gate already covers. Do not describe a
latched single switch as adaptive control.
  transitions: 5 across 5 seed(s) (mean 1.0/run)

 frame   snr(dB)  level   (arm B, seed 0; level tracks the channel)
     0      3.0     L1
    15      7.1     L5
    30     11.1     L8
    45     15.1     L9
    60     19.2     L9
    75     23.2     L9
    90     27.3     L8
   105     28.6     L8
   120     24.6     L9
   135     20.5     L9
   150     16.5     L9
   165     12.5     L7
   180      8.4     L8
   195      4.4     L8
   210     24.0     L6
   225     24.0     L2
   240     24.0     L3
   255     24.0     L2
   270     24.0     L1
   285     24.0     L1

```
</details>

<details>
<summary>Complete candidate standard output</summary>

```text
    Blocking waiting for file lock on build directory
   Compiling coppa-protocol v0.7.0 (/workspace/tmp/catalyst-runner-workspace-T8bUvX/crates/coppa-protocol)
   Compiling coppa-bench v0.7.0 (/workspace/tmp/catalyst-runner-workspace-T8bUvX/crates/coppa-bench)
    Finished `release` profile [optimized] target(s) in 48.06s
     Running `target/release/examples/closed_loop_arq standard`
=== Closed-loop adaptive rate + CP contrast (COP-2) ===
base pair    : hf_standard <-> hf_standard_short_cp (production-faithful, D1)
frames/run   : 300   seeds: [0, 1, 2, 3, 4]   runs: 120

  seed 4 fixed LongCp L1: 293/300 delivered, 320.5 bps
  seed 2 fixed LongCp L1: 293/300 delivered, 320.5 bps
  seed 1 fixed LongCp L1: 296/300 delivered, 323.8 bps
  seed 0 fixed LongCp L1: 296/300 delivered, 323.8 bps
  seed 3 fixed LongCp L1: 292/300 delivered, 319.5 bps
  seed 2 fixed LongCp L2: 285/300 delivered, 651.4 bps
  seed 4 fixed LongCp L2: 291/300 delivered, 665.1 bps
  seed 1 fixed LongCp L2: 290/300 delivered, 662.9 bps
  seed 0 fixed LongCp L2: 282/300 delivered, 644.6 bps
  seed 3 fixed LongCp L2: 281/300 delivered, 642.3 bps
  seed 2 fixed LongCp L3: 277/300 delivered, 1097.4 bps
  seed 1 fixed LongCp L3: 281/300 delivered, 1113.3 bps
  seed 4 fixed LongCp L3: 280/300 delivered, 1109.3 bps
  seed 0 fixed LongCp L3: 283/300 delivered, 1121.2 bps
  seed 3 fixed LongCp L3: 283/300 delivered, 1121.2 bps
  seed 2 fixed LongCp L4: 256/300 delivered, 1543.0 bps
  seed 1 fixed LongCp L4: 260/300 delivered, 1567.2 bps
  seed 4 fixed LongCp L4: 260/300 delivered, 1567.2 bps
  seed 0 fixed LongCp L4: 261/300 delivered, 1573.2 bps
  seed 3 fixed LongCp L4: 265/300 delivered, 1597.3 bps
  seed 2 fixed LongCp L5: 232/300 delivered, 1692.6 bps
  seed 1 fixed LongCp L5: 235/300 delivered, 1714.5 bps
  seed 4 fixed LongCp L5: 236/300 delivered, 1721.8 bps
  seed 0 fixed LongCp L5: 234/300 delivered, 1707.2 bps
  seed 3 fixed LongCp L5: 238/300 delivered, 1736.4 bps
  seed 2 fixed LongCp L6: 244/300 delivered, 1526.4 bps
  seed 1 fixed LongCp L6: 246/300 delivered, 1538.9 bps
  seed 4 fixed LongCp L6: 249/300 delivered, 1557.7 bps
  seed 0 fixed LongCp L6: 241/300 delivered, 1507.6 bps
  seed 3 fixed LongCp L6: 249/300 delivered, 1557.7 bps
  seed 1 fixed LongCp L7: 227/300 delivered, 2160.4 bps
  seed 2 fixed LongCp L7: 232/300 delivered, 2208.0 bps
  seed 0 fixed LongCp L7: 216/300 delivered, 2055.7 bps
  seed 4 fixed LongCp L7: 228/300 delivered, 2169.9 bps
  seed 3 fixed LongCp L7: 231/300 delivered, 2198.5 bps
  seed 2 fixed LongCp L8: 198/300 delivered, 2118.7 bps
  seed 1 fixed LongCp L8: 201/300 delivered, 2150.8 bps
  seed 0 fixed LongCp L8: 201/300 delivered, 2150.8 bps
  seed 3 fixed LongCp L8: 203/300 delivered, 2172.2 bps
  seed 4 fixed LongCp L8: 200/300 delivered, 2140.1 bps
  seed 2 fixed LongCp L9: 160/300 delivered, 2145.5 bps
  seed 1 fixed LongCp L9: 158/300 delivered, 2118.7 bps
  seed 0 fixed LongCp L9: 161/300 delivered, 2158.9 bps
  seed 4 fixed LongCp L9: 159/300 delivered, 2132.1 bps
  seed 3 fixed LongCp L9: 161/300 delivered, 2158.9 bps
  seed 2 fixed ShortCp L1: 290/300 delivered, 362.1 bps
  seed 1 fixed ShortCp L1: 296/300 delivered, 369.6 bps
  seed 0 fixed ShortCp L1: 290/300 delivered, 362.1 bps
  seed 3 fixed ShortCp L1: 292/300 delivered, 364.6 bps
  seed 4 fixed ShortCp L1: 286/300 delivered, 357.1 bps
  seed 1 fixed ShortCp L2: 289/300 delivered, 753.9 bps
  seed 2 fixed ShortCp L2: 285/300 delivered, 743.5 bps
  seed 0 fixed ShortCp L2: 286/300 delivered, 746.1 bps
  seed 3 fixed ShortCp L2: 290/300 delivered, 756.5 bps
  seed 4 fixed ShortCp L2: 282/300 delivered, 735.7 bps
  seed 2 fixed ShortCp L3: 281/300 delivered, 1270.6 bps
  seed 1 fixed ShortCp L3: 283/300 delivered, 1279.7 bps
  seed 4 fixed ShortCp L3: 292/300 delivered, 1320.3 bps
  seed 3 fixed ShortCp L3: 286/300 delivered, 1293.2 bps
  seed 0 fixed ShortCp L3: 278/300 delivered, 1257.0 bps
  seed 1 fixed ShortCp L4: 269/300 delivered, 1850.5 bps
  seed 2 fixed ShortCp L4: 265/300 delivered, 1823.0 bps
  seed 4 fixed ShortCp L4: 270/300 delivered, 1857.4 bps
  seed 3 fixed ShortCp L4: 266/300 delivered, 1829.9 bps
  seed 0 fixed ShortCp L4: 266/300 delivered, 1829.9 bps
  seed 1 fixed ShortCp L5: 247/300 delivered, 2056.7 bps
  seed 2 fixed ShortCp L5: 244/300 delivered, 2031.7 bps
  seed 4 fixed ShortCp L5: 247/300 delivered, 2056.7 bps
  seed 3 fixed ShortCp L5: 248/300 delivered, 2065.0 bps
  seed 0 fixed ShortCp L5: 247/300 delivered, 2056.7 bps
  seed 1 fixed ShortCp L6: 255/300 delivered, 1820.6 bps
  seed 2 fixed ShortCp L6: 249/300 delivered, 1777.8 bps
  seed 4 fixed ShortCp L6: 262/300 delivered, 1870.6 bps
  seed 3 fixed ShortCp L6: 263/300 delivered, 1877.7 bps
  seed 0 fixed ShortCp L6: 252/300 delivered, 1799.2 bps
  seed 1 fixed ShortCp L7: 232/300 delivered, 2520.0 bps
  seed 2 fixed ShortCp L7: 230/300 delivered, 2498.2 bps
  seed 4 fixed ShortCp L7: 236/300 delivered, 2563.4 bps
  seed 0 fixed ShortCp L7: 220/300 delivered, 2389.6 bps
  seed 3 fixed ShortCp L7: 236/300 delivered, 2563.4 bps
  seed 1 fixed ShortCp L8: 203/300 delivered, 2479.1 bps
  seed 2 fixed ShortCp L8: 196/300 delivered, 2393.7 bps
  seed 4 fixed ShortCp L8: 207/300 delivered, 2528.0 bps
  seed 0 fixed ShortCp L8: 204/300 delivered, 2491.4 bps
  seed 3 fixed ShortCp L8: 203/300 delivered, 2479.1 bps
  seed 1 fixed ShortCp L9: 160/300 delivered, 2448.7 bps
  seed 2 fixed ShortCp L9: 158/300 delivered, 2418.1 bps
  seed 4 fixed ShortCp L9: 161/300 delivered, 2464.0 bps
  seed 0 fixed ShortCp L9: 160/300 delivered, 2448.7 bps
  seed 3 fixed ShortCp L9: 159/300 delivered, 2433.4 bps
  seed 2 arm A  : 1977.5 bps
  seed 1 arm A  : 1963.3 bps
  seed 4 arm A  : 1918.6 bps
  seed 0 arm A  : 1880.4 bps
  seed 3 arm A  : 2043.3 bps
  seed 2 arm C  : 2106.1 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 1 arm C  : 2349.6 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 4 arm C  : 2728.5 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 3 arm C  : 2309.6 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 0 arm C  : 2232.6 bps (adaptive rate, fixed short CP -- B's CP-isolation control)
  seed 1 arm C' : 2349.6 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 2 arm C' : 2111.5 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 4 arm C' : 2668.9 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 0 arm C' : 2232.6 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 3 arm C' : 2251.2 bps (arm C, header bandwidth pinned to long-CP's ID 1 -- header-codeword isolation)
  seed 1 arm B  : 2330.3 bps, 1 switches
  seed 4 arm B  : 2704.2 bps, 1 switches
  seed 2 arm B  : 2090.7 bps, 1 switches
  seed 0 arm B  : 2215.2 bps, 1 switches
  seed 3 arm B  : 2291.4 bps, 1 switches
  seed 4 arm B0 : 2715.2 bps, 1 switches
  seed 1 arm B0 : 2339.0 bps, 1 switches
  seed 2 arm B0 : 2097.6 bps, 1 switches
  seed 0 arm B0 : 2223.1 bps, 1 switches
  seed 3 arm B0 : 2299.6 bps, 1 switches
  seed 4 arm P  : 1918.6 bps (1 rebuilds; arm B had 1)
  seed 1 arm P  : 1963.3 bps (1 rebuilds; arm B had 1)
  seed 2 arm P  : 1977.5 bps (1 rebuilds; arm B had 1)
  seed 0 arm P  : 1880.4 bps (1 rebuilds; arm B had 1)
  seed 3 arm P  : 2043.3 bps (1 rebuilds; arm B had 1)

=== PRIMARY: airtime-normalized goodput (bits/s) ===
The `airtime(s)` and `goodput` columns are DATA-AIRTIME-ONLY (= switch cost x0) for EVERY
arm, arm B included. Arm B's CP-handshake air is real and is charged separately in the
SWITCH ACCOUNTING block below (and as the x1 half of every ratio in THE BAR); it is NOT in
this table's denominator. Arms A / P / B0 / C / C' and all 18 comparator cells have no
handshake air at all, so for them x0 is the whole story.
seed   arm       deliv       bits  airtime(s)    goodput     FER   delivery 95% CI
0      A      270/300      353576      188.03     1880.4   10.0%   0.861-0.929
0      P      270/300      353576      188.03     1880.4   10.0%   0.861-0.929
0      B      261/300      352000      158.90     2215.2   13.0%   0.827-0.903
0      B0     261/300      352000      158.34     2223.1   13.0%   0.827-0.903
0      C      261/300      352000      157.66     2232.6   13.0%   0.827-0.903
0      C'     261/300      352000      157.66     2232.6   13.0%   0.827-0.903
0      C(fix)  204/300      257856      103.50     2491.4   32.0%   0.625-0.730    (L8)
1      A      255/300      343496      174.96     1963.3   15.0%   0.805-0.886
1      P      255/300      343496      174.96     1963.3   15.0%   0.805-0.886
1      B      258/300      351152      150.69     2330.3   14.0%   0.816-0.895
1      B0     258/300      351152      150.13     2339.0   14.0%   0.816-0.895
1      C      258/300      351152      149.45     2349.6   14.0%   0.816-0.895
1      C'     258/300      351152      149.45     2349.6   14.0%   0.816-0.895
1      C(fix)  232/300      330368      131.10     2520.0   22.7%   0.723-0.817    (L7)
2      A      260/300      352096      178.05     1977.5   13.3%   0.824-0.901
2      P      260/300      352096      178.05     1977.5   13.3%   0.824-0.901
2      B      265/300      352984      168.84     2090.7   11.7%   0.842-0.915
2      B0     265/300      352984      168.28     2097.6   11.7%   0.842-0.915
2      C      265/300      352984      167.60     2106.1   11.7%   0.842-0.915
2      C'     264/300      351560      166.50     2111.5   12.0%   0.838-0.912
2      C(fix)  230/300      327520      131.10     2498.2   23.3%   0.716-0.811    (L7)
3      A      261/300      357056      174.75     2043.3   13.0%   0.827-0.903
3      P      261/300      357056      174.75     2043.3   13.0%   0.827-0.903
3      B      265/300      359048      156.69     2291.4   11.7%   0.842-0.915
3      B0     265/300      359048      156.13     2299.6   11.7%   0.842-0.915
3      C      265/300      359048      155.46     2309.6   11.7%   0.842-0.915
3      C'     268/300      357624      158.86     2251.2   10.7%   0.853-0.923
3      C(fix)  236/300      336064      131.10     2563.4   21.3%   0.737-0.829    (L7)
4      A      262/300      351640      183.28     1918.6   12.7%   0.831-0.906
4      P      262/300      351640      183.28     1918.6   12.7%   0.831-0.906
4      B      268/300      374528      138.50     2704.2   10.7%   0.853-0.923
4      B0     268/300      374528      137.94     2715.2   10.7%   0.853-0.923
4      C      268/300      374528      137.26     2728.5   10.7%   0.853-0.923
4      C'     267/300      371320      139.13     2668.9   11.0%   0.850-0.921
4      C(fix)  236/300      336064      131.10     2563.4   21.3%   0.737-0.829    (L7)

=== COMPARATORS: 9-cell long-CP-only vs 18-cell joint level x CP (D4) ===
seed    bestfix(long)    lvl bestfix(joint)   cell  oracle(long) oracle(joint)
0              2158.9     L9         2491.4 shortL8        3040.7        3618.2
1              2160.4     L7         2520.0 shortL7        3055.6        3658.3
2              2208.0     L7         2498.2 shortL7        3072.8        3662.8
3              2198.5     L7         2563.4 shortL7        3087.1        3665.8
4              2169.9     L7         2563.4 shortL7        3081.6        3704.1

=== THE BAR (adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8) ===
Each ratio is printed at BOTH switch-cost multipliers: x0 (arm B's data airtime only) and
x1 (the loss-free nominal, one handshake charged per switch). x0 alone is what these rows
used to report, unlabelled -- an arm B that paid nothing for its own switches.
seed          B/bf(long)       B/orc(long)       B/bf(joint)      B/orc(joint)
0            1.026/0.986       0.729/0.700       0.889/0.854       0.612/0.588
1            1.079/1.034       0.763/0.731       0.925/0.887       0.637/0.611
2            0.947/0.912       0.680/0.655       0.837/0.806       0.571/0.550
3            1.042/1.001       0.742/0.713       0.894/0.858       0.625/0.600
4            1.246/1.190       0.878/0.838       1.055/1.008       0.730/0.697
(each cell is x0/x1)

REMINDER (Key Discovery 1): a uniform CP change is ONE CONSTANT airtime divisor, so it
cancels out of any ratio. The `(long)` columns let a short-CP arm bank a spurious
1260/1104 = +14.13% against a comparator DENIED the same lever; the `(joint)` columns are
the honest ones (D4). A `(joint)` miss is NOT evidence against short CP -- the bar is
structurally blind to an airtime lever.
COP-152 seed=0 arm-B bits=352000 data_air=158.89999999999960778 switch_air=6.48700000000000010
COP-152 segment seed=0 segment=2 bits=33088 data_air=33.71800000000001774 switch_air=-0.00000000000000000
COP-152 segment seed=0 segment=3 bits=31424 data_air=47.10399999999998499 switch_air=-0.00000000000000000
COP-152 seed=1 arm-B bits=351152 data_air=150.68899999999962347 switch_air=6.48700000000000010
COP-152 segment seed=1 segment=2 bits=29832 data_air=28.47400000000001441 switch_air=-0.00000000000000000
COP-152 segment seed=1 segment=3 bits=33208 data_air=44.32099999999999795 switch_air=-0.00000000000000000
COP-152 seed=2 arm-B bits=352984 data_air=168.83599999999961483 switch_air=6.48700000000000010
COP-152 segment seed=2 segment=2 bits=32600 data_air=37.12199999999999989 switch_air=-0.00000000000000000
COP-152 segment seed=2 segment=3 bits=32112 data_air=53.72799999999997311 switch_air=-0.00000000000000000
COP-152 seed=3 arm-B bits=359048 data_air=156.69199999999963779 switch_air=6.48700000000000010
COP-152 segment seed=3 segment=2 bits=38184 data_air=31.55600000000002225 switch_air=-0.00000000000000000
COP-152 segment seed=3 segment=3 bits=34352 data_air=47.24199999999998312 switch_air=-0.00000000000000000
COP-152 seed=4 arm-B bits=374528 data_air=138.49899999999971101 switch_air=6.48700000000000010
COP-152 segment seed=4 segment=2 bits=42040 data_air=30.49800000000002242 switch_air=-0.00000000000000000
COP-152 segment seed=4 segment=3 bits=41992 data_air=30.19900000000002294 switch_air=-0.00000000000000000

=== LEGACY CONTROL (airtime-blind bits-per-slot; proves the metric swap changed accounting only) ===
These are the PRE-COP-2 quantities, recomputed. On the `robust` base at seed 0 they must
reproduce the record: adaptive 326680 / best fixed (L4) 357424 / oracle 428936 /
adaptive-oracle 0.762 / adaptive-best-fixed 0.914.
COP-152 legacy seed=0 adaptive=353576 fixed=371664 oracle=436712 bars=None (None = non-decisional)
seed 0   adaptive  353576 bits | best fixed (L4 )  371664 bits | oracle  436712 bits | a/o = 0.810  a/bf = 0.951
COP-152 legacy seed=1 adaptive=343496 fixed=370240 oracle=436104 bars=None (None = non-decisional)
seed 1   adaptive  343496 bits | best fixed (L4 )  370240 bits | oracle  436104 bits | a/o = 0.788  a/bf = 0.928
COP-152 legacy seed=2 adaptive=352096 fixed=364544 oracle=437400 bars=None (None = non-decisional)
seed 2   adaptive  352096 bits | best fixed (L4 )  364544 bits | oracle  437400 bits | a/o = 0.805  a/bf = 0.966
COP-152 legacy seed=3 adaptive=357056 fixed=377360 oracle=438248 bars=None (None = non-decisional)
seed 3   adaptive  357056 bits | best fixed (L4 )  377360 bits | oracle  438248 bits | a/o = 0.815  a/bf = 0.946
COP-152 legacy seed=4 adaptive=351640 fixed=370240 oracle=439312 bars=None (None = non-decisional)
seed 4   adaptive  351640 bits | best fixed (L4 )  370240 bits | oracle  439312 bits | a/o = 0.800  a/bf = 0.950
NOTE: `best fixed` here is an argmax over BITS (the legacy quantity). The primary table's
best-fixed is an argmax over GOODPUT, and the two genuinely select different levels -- that
is the metric change working, not a regression.

=== SWITCH ACCOUNTING (arm B) ===
Charged per switch: 3 frames under the old profile (Propose, Confirm, bare ack) + 2 under
the new one (CpSwitched, its ack) = the 5 droppable on-air frames cp_negotiator counts.
seed    switches   frames   switch_air %of arm B       gp x0       gp x1       gp x6
0              1        5         6.49     4.08%      2215.2      2128.3      1779.4
         switch LongCp -> ShortCp decided@3 effective@9 L1
1              1        5         6.49     4.30%      2330.3      2234.1      1852.0
         switch LongCp -> ShortCp decided@3 effective@9 L1
2              1        5         6.49     3.84%      2090.7      2013.3      1699.0
         switch LongCp -> ShortCp decided@3 effective@9 L1
3              1        5         6.49     4.14%      2291.4      2200.3      1835.5
         switch LongCp -> ShortCp decided@3 effective@9 L1
4              1        5         6.49     4.68%      2704.2      2583.2      2111.0
         switch LongCp -> ShortCp decided@3 effective@9 L1
x0 = no-charge upper bound, x1 = loss-free nominal, x6 = ARQ-budget ceiling
(DEFAULT_MAX_RETRANSMIT = 5). Not charged at all, all biasing arm B optimistic:
leg retransmissions, 4 half-duplex turnarounds at DEFAULT_TURNAROUND = 150 ms, and
handshakes that fail and revert via G1-G4 having spent the full budget for zero CP change.

=== CpGate DELAY-SPREAD HISTOGRAM (arm B, per schedule segment) ===
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up        99       0       0       0       0       0       0       0         1         0
s0 awgn ramp down      95       0       0       0       0       0       0       0         5         0
s0 watterson good      22       9       0       0       0       0       0       0        19         0
s0 watterson poor      24       0       0       0       0      12       0       0        14         0
s1 awgn ramp up        99       0       0       0       0       0       0       0         1         0
s1 awgn ramp down      96       0       0       0       0       0       0       0         4         0
s1 watterson good      14      13       0       0       0       0       0       0        23         0
s1 watterson poor      20       0       0       0       0      16       0       0        14         0
s2 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s2 awgn ramp down      95       0       0       0       0       0       0       0         5         0
s2 watterson good      25       6       0       0       0       0       0       0        19         0
s2 watterson poor      23       0       0       0       0      16       0       0        11         0
s3 awgn ramp up        99       0       0       0       0       0       0       0         1         0
s3 awgn ramp down      96       0       0       0       0       0       0       0         4         0
s3 watterson good      28       6       0       0       0       0       0       0        16         0
s3 watterson poor      24       0       0       0       0      12       0       0        14         0
s4 awgn ramp up        99       0       0       0       0       0       0       0         1         0
s4 awgn ramp down      97       0       0       0       0       0       0       0         3         0
s4 watterson good      24      11       0       0       0       0       0       0        15         0
s4 watterson poor      17       0       0       0       0      20       0       0        13         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)
COP-152 standard arm-B-x1 operands=[[2128.341405309975, 2491.3623188405863, 3618.2186707233354], [2234.1324375222734, 2519.9694889397483, 3658.3252238956748], [2013.3353866862922, 2498.2456140350955, 3662.812316559057], [2200.332150583107, 2563.4172387490544, 3665.815170102382], [2583.201136661476, 2563.4172387490544, 3704.09838739494]] bars=Some((false, false)) (None = non-decisional)

=== TICKET COP-2 DELTA ===
Both answers below were PRE-COMMITTED in the plan (D4a) before this run.

PRIMARY -- the denominator delta (where a uniform airtime saving actually appears):
  best-fixed:  long-CP-only    2179.1 bps  ->  joint    2527.3 bps   (+15.98%)
  oracle    :  long-CP-only    3067.6 bps  ->  joint    3661.9 bps   (+19.37%)
  (the uniform CP factor is 1260/1104 = +14.130%; a value at or near it means the lever is
working exactly as arithmetic predicts, NOT that the bar moved)

SECONDARY -- does CP *adaptivity* add anything over fixed short CP?
  arm B (CP-adaptive)    2326.4 bps   vs   arm C (adaptive rate, fixed short CP)    2345.3 bps  (-0.81%)   [x0]
  arm B (CP-adaptive)    2231.9 bps   vs   arm C (adaptive rate, fixed short CP)    2345.3 bps  (-4.84%)   [x1]
  arm B / best-fixed(joint) = 0.921 (x0) / 0.883 (x1)  (bar: > 1.0)
  arm B / oracle(joint)     = 0.635 (x0) / 0.609 (x1)  (bar: >= 0.8)
  x0 = arm B's data airtime only; x1 = its handshake air charged once per switch. Arm C
has no handshake air at all, so only arm B's side of each comparison moves.
  upper bound (NOT a CP-isolation control): best hindsight-picked FIXED short-CP cell
     2527.3 bps -- fixed rate AND fixed CP, so B/this delta still conflates both adaptive
  dimensions; kept for reference only, per-frame in the seed table above as row "C".

HEADER-CODEWORD ISOLATION (arm C vs arm C', same short CP, header bandwidth ID only):
  arm C (header id 4)    2345.3 bps   vs   arm C' (header id 1)    2322.8 bps  (-0.96%)
  for scale: B-vs-C was -0.81% (x0). If |C-vs-C'| is comparable to |B-vs-C|, the
CP-specific conclusion above is confounded by the header codeword and should not be
drawn without controlling for it.

D9 DECISION RULE -- per-seed paired deltas, decided on SIGN CONSISTENCY:
Deltas are shown at x0 and x1. The sign counts below are tallied at BOTH, because a sign
that survives x0 but not x1 is a result the no-charge accounting manufactured.
seed         B-A (bps) x0/x1       B-C (bps) x0/x1       B/bf(joint)
0              +334.8/+247.9          -17.4/-104.2       0.889/0.854
1              +367.0/+270.8          -19.3/-115.4       0.925/0.887
2               +113.2/+35.9           -15.4/-92.8       0.837/0.806
3              +248.1/+157.0          -18.2/-109.3       0.894/0.858
4              +785.6/+664.6          -24.3/-145.3       1.055/1.008
  x0: B > A on 5/5 seeds; B > C on 0/5 seeds.
  x1: B > A on 5/5 seeds; B > C on 0/5 seeds.
  Sign-consistent (all 5 seeds one way) => a DIRECTIONAL result. Mixed signs => no result,
and the write-up must say the delta is inside this bench's own run-to-run band rather than
reporting the mean as if it were an effect. Pairing is exact only at the CHANNEL level: once
delivery outcomes diverge, RateLoop's trajectory diverges too, so the arms do not transmit
the same levels on the same frames.

REBUILD PLACEBO -- arm A 1956.6 bps vs arm P 1956.6 bps (+0.00%)
  If these differ by more than this bench's run-to-run band, arm B must be read against P,
not A, and the write-up must say so. Do NOT assume the transceiver rebuild is inert.

D1a SHORT-CP AIRTIME SHARE (arm B, on the control arm's pre-reduction basis):
  seed 0   levels<=4  46.71%   levels>=5  47.75%   (total short-CP  94.46%)
  seed 1   levels<=4  38.81%   levels>=5  55.34%   (total short-CP  94.15%)
  seed 2   levels<=4  52.48%   levels>=5  42.31%   (total short-CP  94.79%)
  seed 3   levels<=4  43.26%   levels>=5  51.12%   (total short-CP  94.38%)
  seed 4   levels<=4  30.03%   levels>=5  63.61%   (total short-CP  93.63%)
  Only the levels<=4 share is production-reachable: `select_ofdm_profile` routes every level
>= 5 to `vhf_wide()` and ignores cp_mode there, so the levels>=5 share is an explicit
UPPER-BOUND OVERSHOOT this bench collects and production cannot (D1a).

TRANSITION-COUNT GATE (pre-committed): if arm B yields <= 1 CpGate transition, this
schedule does not exercise CP *adaptivity* at all and the only content is the arm B vs
arm C fixed contrast -- which short_cp_fading_gate already covers. Do not describe a
latched single switch as adaptive control.
  transitions: 5 across 5 seed(s) (mean 1.0/run)

 frame   snr(dB)  level   (arm B, seed 0; level tracks the channel)
     0      3.0     L1
    15      7.1     L5
    30     11.1     L9
    45     15.1     L9
    60     19.2     L9
    75     23.2     L9
    90     27.3     L9
   105     28.6     L9
   120     24.6     L9
   135     20.5     L9
   150     16.5     L9
   165     12.5     L9
   180      8.4     L9
   195      4.4     L8
   210     24.0     L5
   225     24.0     L2
   240     24.0     L4
   255     24.0     L3
   270     24.0     L2
   285     24.0     L3

```
</details>

### Disposition

Complete output blocks above omit trailing spaces only.

COP-152 is incomplete. The frozen candidate misses both full comparisons and
regresses standard Poor-segment pooled goodput by 19.96%, despite passing both
held-out screens. Production adoption was withdrawn. The pilot recommendation is
computed once by the common receive core for both public wrappers. The bounded
prefix measurement and its regression/grid tests remain experimental, with no
production reconstruction cost. Candidate CPU and full-loop numbers above describe
the rejected integration, not the final pilot-based tree. The final production
recommendations and transmitted samples match the paired baseline, so no second
candidate run is claimed after withdrawal. Further receiver redesign needs a new
evidence-backed plan; no thresholds or controller parameters were relaxed here.

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
production floor waveform those sweeps must select does not exist yet.

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
tracking or prove reception between grid points. The BPSK comparison, production
selection and integration, session support and final sweep/connect/ramp evidence
remain unbuilt. No acceptance criterion is declared complete by this repair.
=======
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
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b
