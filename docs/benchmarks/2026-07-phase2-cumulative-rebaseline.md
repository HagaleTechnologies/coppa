> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Phase 2 (parametric estimation + NR BG2): cumulative re-baseline

This is Task 8's phase-gate measurement — the full 400-trial/point sweep across every
measurable speed level (1-7, 9, 10; level 8 is reserved 32-QAM) for AWGN, Watterson
Good/Moderate/Poor, and the `--ssb`/`--cfo 40` bench flags, generated after all seven prior
Phase 2 dev tasks (delay-domain estimator, soft header, LLR calibration + known-pad pinning,
NR BG2 LDPC, turbo re-estimation, fast demappers, Kalman tracker) had landed. It measures the
**cumulative** effect of all six shipped Phase 2 codec changes against the Phase 1 exit
baseline (`results/p1-hotfix/{awgn,moderate,poor}.csv`) — the same baseline `docs/adr/004` and
docs/LIMITATIONS.md's sparse-pilot-regression fix cite as "Phase 1 exit."

**Headline finding, stated up front, per this task's explicit instruction to report exactly
what is measured:** three of Phase 2's seven dev tasks (delay-domain estimator, Kalman
tracker, NR BG2 LDPC) did not meet their own individual acceptance bars, and two of those left
a real, unresolved *regression* in isolated testing. The cumulative full-ladder result below is
a genuine mixed bag, consistent with that — some levels/channels improve substantially (driven
by the soft header, LLR calibration, and turbo re-estimation's real wins), a few show no clear
net change, and a few show a real net regression. **The phase's own stated acceptance bar
(≥+3 dB at FER@10%-CI on watterson-poor/level 2, ≥+1.5 dB at level 6, AWGN ladder ≥+1 dB, header
failures <10% of residual on poor) is not cleanly met — see "Acceptance summary" below for
exactly where and by how much.**

**Two further findings are the single most severe results in this cumulative sweep, and are
called out here up front rather than left to a body subsection:** under a 40 Hz carrier
frequency offset, level 4 (QPSK 3/4) collapses from clean convergence (0/400 errors by 18 dB in
Phase 1) to a flat ~46-52% FER floor across the *entire* 9-30 dB range in Phase 2 — a hard floor
that does not improve with more SNR at all, not just reduced margin (see "`--cfo 40` sweep"
below).

> **UPDATE (2026-07-08): the CFO×level-4 floor above is FIXED**, by
> `CoppaModem::bounded_coarse_delay` (commit `5e7ba93`), independently reverified safe under
> combined CFO+Watterson-fading conditions (commit `daf1d45`). See the "UPDATE" note in the
> "`--cfo 40` sweep" section below and the Acceptance summary table for the fixed numbers.

To a lesser but still real degree, Watterson-Moderate levels 3-6 (QPSK, 8PSK, 16QAM 1/2)
regress at matched operating SNR relative to the Phase 1 exit baseline (see "Watterson Moderate"
below). Neither of these was part of the phase's original four stated acceptance criteria, but
both are real findings from this same measurement that belong in the acceptance summary, not
just in body prose — both are added there as additional rows below.

Command: `cargo run -p coppa-bench --release -- --channel {awgn|good|moderate|poor} --trials 400
[--ssb] [--cfo 40]` (seed `0x00C0FFEE`, the CLI default). Raw CSVs:
`results/p2-final/{awgn,good,moderate,poor}.csv`, `results/p2-final-ssb/awgn.csv`,
`results/p2-final-cfo40/awgn.csv`.

### AWGN — the codec-swap ladder, ≥+1 dB bar: met and exceeded, with two minor tail exceptions

| Mode (level) | Phase 1 exit FER≤10%/≤1% | Phase 2 FER≤10%/≤1% | Delta (FER≤10%) | Peak goodput: before → after |
|---|---|---|---|---|
| BPSK 1/4 (1) | 6.0 / 9.0 dB | 6.0 / 9.0 dB | unchanged | 352 → 352 |
| BPSK 1/2 (2) | 6.0 / 9.0 dB | 6.0 / 9.0 dB | unchanged | 709 → 709 |
| QPSK 1/2 (3) | 6.0 / 9.0 dB | 6.0 / 9.0 dB | unchanged | 1229 → 1229 |
| QPSK 3/4 (4) | 9.0 / 12.0 dB | **6.0 / 9.0 dB** | **+3 dB (better)** | 1849 → 1849 |
| 8PSK 2/3 (5) | 15.0 / 15.0 dB | 15.0 / 15.0 dB | unchanged | 5082 → 5082 |
| 16QAM 1/2 (6) | 15.0 / 15.0 dB | 15.0 / **18.0** dB | unchanged (≤1% 3 dB worse) | 4555 → 4555 |
| 16QAM 3/4 (7) | 18.0 / 18.0 dB | **15.0** / 18.0 dB | **+3 dB (better)** | 6852 → 6852 |
| 64QAM 2/3 (9) | 24.0 / 27.0 dB | **18.0** / never | **+6 dB (better)**, but ≤1% floor appears (~1-1.25% FER, 18-30 dB) | 6776 → 6709 |
| 64QAM 5/6 (10) | never clears | **24.0 / 27.0 dB** | **dramatic fix** (was rate 7/8, non-convergent) | 399 → **8450** |

**Met, in aggregate, by a wide margin.** Level 4 gains a clean +3 dB (the LLR-calibration/
known-pad-pinning gain surfacing here, unmasked by sync since level 4 uses the fuller-payload
regime). Level 7 gains +3 dB and level 9 gains +6 dB at FER≤10% — both consistent with Task 4's
own isolated finding that the coding gain is real, if smaller than predicted, and largest for
the highest-rate/previously-weakest levels. **Level 10 is the standout win**: the rate-7/8→5/6
change (ADR-005) fixes the pre-Phase-2 non-convergence entirely — peak goodput goes from 399
to 8450 bps, a >20x increase, because the old 7/8 code essentially never decoded above the
noise floor.

**Two small, real exceptions, not hidden**: levels 6 and 9 each show their FER≤1% threshold get
3 dB *worse* (level 6) or undefined (level 9, a residual ~1-1.25% error floor that persists to
30 dB where the old codec hit 0/400 errors by 27 dB). Both are small in absolute terms (a few
failures per 400 trials) and consistent with Task 4's own finding that the new mother code's
gain is real but modest at matched block length — not investigated further here, flagged for
whoever revisits the LDPC layer next.

### Watterson Good — standalone (no Phase-1-exit baseline exists for this channel; compared to `results/p1-final/good.csv`)

| Mode (level) | Phase 1 final FER≤10%/≤1% | Phase 2 FER≤10%/≤1% | Peak goodput: before → after |
|---|---|---|---|
| BPSK 1/4 (1) | 27.0 / never | **12.0** / never | 327 → 350 |
| BPSK 1/2 (2) | 21.0 / never | **12.0** / never | 665 → 699 |
| QPSK 1/2 (3) | never / never | never / never | 983 → 1091 |
| QPSK 3/4 (4) | never / never | never / never | 740 → **1160** (+57%) |
| 8PSK 2/3 (5) | 21.0 / never | 21.0 / **30.0** | 5070 → 5082 |
| 16QAM 1/2 (6) | 21.0 / 30.0 | 21.0 / 30.0 | 4555 → 4555 (identical) |
| 16QAM 3/4 (7) | never / never | never / never | 5790 → 5995 |
| 64QAM 2/3 (9) | never / never | never / never | 1372 → **2168** (+58%) |
| 64QAM 5/6 (10) | never / never | never / never | 22 → 296 (both still marginal) |

**A clean win for the low-order HF-profile modes** (levels 1-2 improve 9-15 dB at FER≤10%) and
real goodput gains for most other levels; level 6 (16QAM, VHF profile) is bit-for-bit
unchanged.

### Watterson Moderate — a genuinely mixed cumulative result

| Mode (level) | Phase 1 exit FER≤10%/≤1% | Phase 2 FER≤10%/≤1% | Delta | Peak goodput: before → after |
|---|---|---|---|---|
| BPSK 1/4 (1) | 18.0 / never | **12.0** / never | **+6 dB (better)** | 341 → 350 |
| BPSK 1/2 (2) | 18.0 / never | **15.0** / never | **+3 dB (better)** | 686 → 688 |
| QPSK 1/2 (3) | never / never | never / never | undefined — **regresses at matched SNR** (30 dB FER 8.0%→17.0%) | 1131 → 1026 |
| QPSK 3/4 (4) | never / never | never / never | undefined — **regresses at matched SNR** (30 dB FER 38.5%→41.5%; 24 dB 33.25%→43.5%) | 1234 → 1082 |
| 8PSK 2/3 (5) | never / never | never / never | undefined — **regresses substantially at matched SNR** (27 dB FER 23.25%→50.25%) | 3901 → 2783 |
| 16QAM 1/2 (6) | never / never | never / never | undefined — **regresses substantially at matched SNR** (30 dB FER 10.25%→40.0%) | 4088 → 2733 |
| 16QAM 3/4 (7) | never / never | never / never | undefined — **improves at matched SNR** (30 dB FER 76.75%→68.75%) | 1593 → 2141 |
| 64QAM 2/3 (9) | never / never | never / never | undefined — **improves at matched SNR** (30 dB FER 93.25%→85.0%) | 457 → 1017 |
| 64QAM 5/6 (10) | never (0 bps) | never | unchanged (both non-functional) | 0 → 21 |

**This is the clearest evidence of the mixed cumulative picture the task brief anticipated.**
Levels 1-2 (BPSK, `hf_standard`) show a real, clean win at FER≤10% (+6 dB and +3 dB
respectively) — consistent with turbo re-estimation's benefit being concentrated on BPSK (Task
5's report: 21-50% of first-pass failures rescued at level 2) outweighing Task 1/7's estimator
regression there. **Levels 3-6 (QPSK and 8PSK/16QAM 1/2) instead show a real regression at
matched operating SNR** — FER is *worse*, not just "unmet metric," at the mid-to-high SNR
points that matter most, most severely for levels 5/6 (roughly doubling FER at 27-30 dB).
Levels 7 and 9 (16QAM 3/4, 64QAM 2/3) improve consistently at every SNR point, matching the
AWGN-ladder pattern of the new LDPC code's largest gains landing on the previously-weakest,
highest-rate levels. This pattern — BPSK wins, mid-ladder QPSK/8PSK/16QAM-1/2 regresses,
top-of-ladder 16QAM-3/4/64QAM wins — is new information this phase-gate sweep surfaces; no
single dev task's own bench gate swept enough of the ladder simultaneously to see it.

### Watterson Poor — the phase's own primary acceptance channel

| Mode (level) | Phase 1 exit FER≤10%/≤1% | Phase 2 FER≤10%/≤1% | Peak goodput: before → after |
|---|---|---|---|
| BPSK 1/4 (1) | never / never | never / never | 233 → 318 |
| BPSK 1/2 (2) | never / never | never / never | 404 → 560 |
| QPSK 1/2 (3) | never / never | never / never | 738 → 940 |
| QPSK 3/4 (4) | never / never | never / never | 555 → 989 |
| 8PSK 2/3 (5) | never / never | never / never | 1537 → 1245 |
| 16QAM 1/2 (6) | never / never | never / never | 1264 → 1139 |
| 16QAM 3/4 (7) | never / never | never / never | 480 → 651 |
| 64QAM 2/3 (9) | never / never | never / never | 51 → 237 |
| 64QAM 5/6 (10) | never (0 bps) | never (21 bps) | 0 → 21 |

**The literal FER≤10% threshold metric never crosses for any level, before or after** — Poor is
(and remains) an irreducible-outage-floor channel at this trial count, matching every prior Task
1/5/7 report's finding. Per this project's practice of not letting an undefined threshold hide
a real result, the underlying FER-vs-SNR curves (`results/p1-hotfix/poor.csv` vs
`results/p2-final/poor.csv`, full data) tell a much more informative story than "undefined" at
every level:

| Level | FER at 30 dB, before → after | Direction |
|---|---|---|
| 1 (BPSK 1/4) | 36.75% → **9.50%** | **large, real improvement** (nearly clears 10% outright) |
| 2 (BPSK 1/2) | 44.25% → **21.75%** | **large, real improvement** (roughly halved) |
| 3 (QPSK 1/2) | 44.75% → **26.75%** | **real improvement** |
| 4 (QPSK 3/4) | 71.75% → **48.50%** | **real improvement** |
| 5 (8PSK 2/3) | 75.00% → 75.75% | flat (regresses at 24-27 dB: 69.75%→75.5%) |
| 6 (16QAM 1/2) | 74.00% → 76.25% | flat / marginally worse |
| 7 (16QAM 3/4) | 94.00% → 93.25% | slight improvement |
| 9 (64QAM 2/3) | 99.25% → 96.50% | improvement (goodput 4.7x) |
| 10 (64QAM 5/6) | 100% → ~99.75% | both non-functional |

**Level 2 — the phase's own named acceptance point — shows a real, large, substantially-more-
than-3-dB-equivalent improvement in the underlying FER (roughly halved at every SNR from 12-30
dB), but because neither the before nor after state ever crosses the 10% Wilson-CI line, the
literal "≥+3 dB at FER@10%-CI" metric cannot be evaluated as stated — it is undefined, not
failing, exactly as Tasks 1/5/7 already found for this same channel/level.** Reported honestly:
the metric as literally specified is not met (not measurable), but the underlying physical
quantity it was meant to proxy shows a real, substantial, verified gain.

**Level 6 — the phase's other named acceptance point — shows no such gain.** The FER curve is
essentially flat (no net improvement) from 18 dB up, with results at individual SNR points
within trial noise of each other — two of the five points from 18 dB up are actually
marginally *better* (18 dB: 88.75%→87.5%; 24 dB: 79.0%→77.75%), while 21/27/30 dB are
marginally worse (30 dB: 74.0%→76.25%), consistent with Task 5's own finding that turbo
re-estimation rescues only 1-2% of first-pass failures on level 6/Watterson-Poor (vs.
21-50% at level 2), leaving Task 1/7's estimator regression comparatively unmasked at this
level. **The ≥+1.5 dB level-6 bar is not met, in either its literal (undefined-threshold) or
underlying (essentially-flat, no-net-improvement FER) sense.**

### `--ssb` sweep (AWGN + realistic SSB rig passband, all levels)

Phase 1 column cites `results/p1-final-ssb/awgn.csv` — the pre-hotfix baseline (same
substitution as the Good-channel table above, since no `p1-hotfix`-generation SSB sweep exists).
This is immaterial here for the same reason it's immaterial for Good: the interleaver/LDPC-clamp
hotfix only touched Watterson-fading sync timing, not the AWGN+SSB path this sweep exercises.

VHF-profile levels (5-10) fail completely under `--ssb` in both the Phase 1 and Phase 2
codecs — expected, not a defect (`vhf_wide`'s ~350-5900 Hz occupied band is far wider than the
300-2700 Hz SSB emulation; this combination is out of scope by design, same finding as Phase
1's own gate). HF-profile levels (1-4), the only ones this test meaningfully exercises:

| Mode (level) | Phase 1 `--ssb` FER≤10%/≤1% | Phase 2 `--ssb` FER≤10%/≤1% |
|---|---|---|
| BPSK 1/4 (1) | 6.0 / 9.0 dB | 6.0 / **12.0** dB (≤1% 3 dB worse) |
| BPSK 1/2 (2) | 6.0 / 12.0 dB | 6.0 / 12.0 dB (unchanged) |
| QPSK 1/2 (3) | 6.0 / 9.0 dB | 6.0 / 9.0 dB (unchanged) |
| QPSK 3/4 (4) | 9.0 / 12.0 dB | **6.0 / 9.0 dB** (+3 dB better) |

Matches the AWGN ladder's own pattern closely (level 4 real gain, a couple of small FER≤1%-only
regressions) — the SSB emulation doesn't introduce any new Phase-2-specific effect.

### `--cfo 40` sweep (AWGN + 40 Hz carrier offset, all levels)

Phase 1 column cites `results/p1-final-cfo40/awgn.csv` — the pre-hotfix baseline (same
substitution as the Good-channel table above, since no `p1-hotfix`-generation CFO sweep exists).
This is immaterial here for the same reason it's immaterial for Good: the interleaver/LDPC-clamp
hotfix only touched Watterson-fading sync timing, not the AWGN+CFO path this sweep exercises.

| Mode (level) | Phase 1 `--cfo 40` FER≤10%/≤1% | Phase 2 `--cfo 40` FER≤10%/≤1% |
|---|---|---|
| BPSK 1/4 (1) | 6.0 / 9.0 dB | 6.0 / 9.0 dB (unchanged) |
| BPSK 1/2 (2) | 6.0 / 9.0 dB | 6.0 / 9.0 dB (unchanged) |
| QPSK 1/2 (3) | 6.0 / 9.0 dB | 9.0 / 18.0 dB (**3-9 dB worse**) |
| QPSK 3/4 (4) | 15.0 / 18.0 dB | **never clears** (peak goodput 1849→994) |
| 8PSK 2/3 (5) | 15.0 / 15.0 dB | 15.0 / 15.0 dB (unchanged) |
| 16QAM 1/2 (6) | 15.0 / 15.0 dB | 15.0 / 18.0 dB (≤1% 3 dB worse) |
| 16QAM 3/4 (7) | 18.0 / 18.0 dB | 15.0 / 18.0 dB (≤10% 3 dB better) |
| 64QAM 2/3 (9) | 24.0 / 27.0 dB | 21.0 / never (≤10% 3 dB better, ≤1% undefined) |
| 64QAM 5/6 (10) | never (0 bps) | 24.0 / 30.0 dB (dramatic fix, matches AWGN) |

**A new, real regression surfaces here, not present in the plain-AWGN or `--ssb` sweeps**:
level 4 (QPSK 3/4) under a 40 Hz carrier offset goes from clearing FER≤10%/≤1% at 15/18 dB to
**never clearing at all**, with peak goodput dropping by 46% (1849→994 bps). Level 3 also costs
3-9 dB more margin than before. Levels 1/2/5/6 are flat-to-marginally-different (matching the
AWGN pattern); levels 7/9/10 show the same real gains AWGN shows. This CFO×level-4 interaction
was not exercised by any single Phase 2 dev task's own bench gate (none of them swept `--cfo`)
and was new information from this phase-gate sweep.

**UPDATE (2026-07-08): level 4's CFO floor is FIXED — see the "UPDATE" note directly below**,
in the detailed mechanism/measurement writeup that follows this table.

**A plausible (not proven) mechanism**: a static CFO offset is exactly the kind of fixed
phase-ramp that Task 1's frame-global `calibrated_bias` reference could mismodel. That
reference is derived once, at `CoppaModem::new()`, via a clean-channel (no propagation
impairment) self-calibration frame, and is then reused unchanged for every frame afterward
(see "Watterson Moderate" above and Task 1's own report for the same frame-global-reference
mechanism implicated there). A persistent CFO-induced phase drift across the frame is exactly
the kind of effect that construction-time calibration against a static reference has no way to
know about or correct for — plausibly compounding with level 4's already-tighter LLR margins
(QPSK 3/4, the highest-rate HF-profile mode) to produce the flat floor. Not investigated
further here.

**UPDATE (2026-07-08): this speculated mechanism was confirmed correct, and the floor is
FIXED.** A follow-up investigation (`.superpowers/sdd/p2-cfo-level4-investigation-report.md`)
directly measured the mechanism above rather than leaving it speculative: a nonzero CFO induces
several-sample sync-timing jitter in `SyncDetector`'s strongest-path `frame_start` (worst
observed `|delta|` from `calibrated_bias` ≈ 0.36 grid units, at CFO=39/39.5 Hz — exactly the
worst-FER CFO values), which the single frame-global `calibrated_bias` cannot represent; the
mismatch leaks real probe energy into a spurious second delay-domain tap, corrupting the LDPC
input LLRs identically at every level, and level 4's tight rate-3/4 margin is the first to lose
convergence outright (confirmed deterministic: near-identical fitted taps/`noise_var` across 10
independent seeds at the same CFO — not an SNR effect, which is why more SNR never helped).
CFO estimation itself, turbo re-estimation, and LDPC alpha/iteration tuning were all directly
ruled out as the cause (see the investigation report).

The fix (`.superpowers/sdd/p2-cfo-level4-fix-report.md`, commit `5e7ba93`) adds
`CoppaModem::bounded_coarse_delay`, which clamps the per-frame coarse-delay correction to
`±COARSE_DELAY_JITTER_BOUND = 0.15` grid units instead of trusting the raw per-frame estimate —
small enough that it cannot reach the multi-grid-unit swings a real two-tap Watterson channel
produces (the failure mode that broke the earlier fully-adaptive attempt referenced in
`docs/adr/004-strongest-path-timing.md`), but large enough to fully clear the CFO floor (the
minimal sufficient bound measured at ≈0.07). Before/after, level 4, `--cfo 40`-band, 30 dB, AWGN:

| CFO (Hz) | Before (unmodified) FER | After (bound=0.15) FER |
|---|---|---|
| 0 | 0.000 | 0.000 |
| 33-38 | 0.017-0.067 | 0.000 |
| 39 | **1.000** | 0.000 |
| 39.5 | **1.000** | 0.000 |
| 40 | **0.417** | 0.000 |
| 40.5-45 | 0.000-0.033 | 0.000 |

Full 30-50 Hz band at 1 Hz steps, 80 trials/point, post-fix: all-zero. SNR sweep at the
worst pre-fix point (CFO=39.5) shows a near-complete recovery of the clean SNR-response curve
(cost under 3 dB, full recovery by ~9 dB, vs. the unfixed floor's total non-response to SNR at
all). The exact regression test this code area previously broke
(`hf_standard_header_survives_watterson_moderate_fading`) still passes, and a precise
(not just pass/fail) pass-rate comparison shows the bound's cost against Watterson-Moderate is
within trial noise: 98.33-98.67% (bound=0.15) vs. 98.78-99.00% (baseline) at n=300-900, a
0.3-0.5 percentage-point gap against a ≈0.4% binomial std (~1.2σ) — not a reproducible
regression. A broader levels-1-4 × Good/Moderate/Poor sanity sweep (48 points) found no other
regression (mean diff ≈ 0, no systematic direction). A later post-merge review
(commit `daf1d45`) additionally swept the untested *combined* CFO+Watterson-fading case
directly (level 4, Watterson-Moderate, CFO∈{0, 39.5, 40} Hz × 6-30 dB) and confirmed the same
result holds jointly: the AWGN-only CFO floor does not reappear under real fading, and the
fixed-vs-pre-fix diff under the combined condition is ~0 (mean −0.011 FER). Full test suite,
clippy, and fmt all clean after the fix.

### Header failure share on Poor (soft Golay) — `header_diagnostic`, 100 trials/cell

Levels 2/3/6 (BPSK/QPSK/16QAM 1/2), robust profile, 6/12/18/24/30 dB, per-cell attribution of
every dropped frame to header-CRC-failure vs. genuine payload limit (see
`crates/coppa-bench/examples/header_diagnostic.rs`'s own doc comment for the exact attribution
rule). Reproduce with `cargo run --release -p coppa-bench --example header_diagnostic`.

| Level | Total frame failures (5 SNR points) | of which header-caused | Share |
|---|---|---|---|
| 2 (BPSK 1/2) | 134 | 15 | 11.2% |
| 3 (QPSK 1/2) | 180 | 10 | 5.6% |
| 6 (16QAM 1/2) | 263 | 0 | 0.0% |
| **All three, all SNRs** | **577** | **25** | **4.3%** |

The header-caused share is concentrated entirely at the two lowest SNR points (6 dB, 12 dB);
from 18 dB up, every level/channel cell is at or below 4%:

| Level | 6 dB | 12 dB | 18 dB | 24 dB | 30 dB |
|---|---|---|---|---|---|
| 2 | 26.7% | 14.8% | 3.8% | 3.8% | 4.0% |
| 3 | 14.3% | 2.7% | 2.9% | 2.9% | 3.0% |
| 6 | 0.0% | 0.0% | 0.0% | 0.0% | 0.0% |

**Aggregate across all measured cells: 4.3% — comfortably under the 10% bar.** At the two
lowest SNR points (6/12 dB, where the payload is failing 30-60% of the time to begin with —
see the Poor FER tables above) the header's own share briefly exceeds 10% (up to 26.7% for
level 2 at 6 dB), but the absolute failure counts there are still dominated by the payload, and
from 18 dB up the soft header contributes essentially none of the residual loss at any of the
three levels tested. **This criterion is met in aggregate**, with the low-SNR caveat reported
honestly rather than folded away.

### Acceptance summary

| Criterion | Status |
|---|---|
| AWGN ladder ≥ +1 dB from the code swap | **Met and exceeded** (level 4 +3 dB, level 7 +3 dB, level 9 +6 dB at FER≤10%; level 10 fixed from non-convergent to clearing cleanly) — with two small, real FER≤1%-only exceptions (levels 6, 9) |
| Watterson-poor/level 2 ≥ +3 dB at FER@10%-CI | **Not measurable as literally specified** — neither the Phase 1 nor Phase 2 codec ever crosses the 10% Wilson-CI bound on Poor at level 2. The underlying FER is substantially better at every SNR (roughly halved at 30 dB: 44.25%→21.75%), a real, large, verified gain — but the specific dB-shift metric the bar names cannot be computed |
| Watterson-poor/level 6 ≥ +1.5 dB at FER@10%-CI | **Not met.** Also not measurable by the literal metric (never crosses 10%), and unlike level 2, the underlying FER shows no net improvement — essentially flat from 18 dB up, with individual SNR points within trial noise of each other (18/24 dB marginally better, 21/27/30 dB marginally worse) |
| Header failures < 10% of residual frame failures on poor (soft Golay) | **Met in aggregate** (4.3% across levels 2/3/6, all SNRs); briefly exceeds 10% only at the two lowest SNR points tested (6/12 dB), where the payload itself is already failing 30-60% of the time — from 18 dB up, every cell is ≤4% |
| *(not an original criterion, added here as a same-measurement finding)* CFO×level-4 interaction | **FIXED (2026-07-08, was: new severe regression).** Under a 40 Hz carrier offset, level 4 (QPSK 3/4) went from clean convergence (0/400 errors by 18 dB in Phase 1) to a flat ~46-52% FER floor across the entire 9-30 dB range in Phase 2 — a hard ceiling that did not recede with more SNR at all. Root-caused to CFO-induced sync jitter desyncing the frame-global `calibrated_bias` reference; fixed by `CoppaModem::bounded_coarse_delay` (commit `5e7ba93`), which restores FER→0.00 across the whole CFO band at a cost within trial noise against Watterson-fading (~1.2σ), independently reverified safe under combined CFO+Watterson-fading conditions (commit `daf1d45`). See "`--cfo 40` sweep" below |
| *(not an original criterion, added here as a same-measurement finding)* Watterson-Moderate levels 3-6 | **Real regression at matched SNR.** QPSK 1/2 (3), QPSK 3/4 (4), 8PSK 2/3 (5), and 16QAM 1/2 (6) genuinely regress at the SNRs that matter most relative to the Phase 1 exit baseline — most severely levels 5/6, roughly doubling FER at 27-30 dB (level 5: 23.25%→50.25% at 27 dB; level 6: 10.25%→40.0% at 30 dB). See "Watterson Moderate" below |

**Overall: the phase's own stated acceptance bar is not cleanly met.** The AWGN criterion is met
and exceeded; the two Watterson-Poor criteria are not met, one because the literal metric is
undefined given Poor's irreducible outage floor (with a real, substantial underlying gain at
level 2) and the other because level 6 genuinely shows no net improvement. This is consistent
with, not contrary to, what was already known going into this gate: Tasks 1 and 7 left a real,
unresolved channel-estimation regression, and Task 4's LDPC coding gain fell short of its own
target — both of which land hardest on exactly the higher-order-modulation/Watterson-fading
cells (levels 3-6 on Moderate, level 6 on Poor) where this gate's own acceptance bar is aimed.
The clear, unambiguous wins this phase delivered — soft header, LLR calibration, turbo
re-estimation's rescue of BPSK, the level-10 LDPC fix — are real and substantial, but they do
not fully offset the estimator regression and LDPC coding-gain shortfall everywhere the
acceptance bar checks.

