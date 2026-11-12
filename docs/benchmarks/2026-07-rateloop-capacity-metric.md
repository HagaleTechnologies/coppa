> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07-28 — RateLoop capacity-metric same-frame accuracy: ground-truthed, root-caused as a
## fading-coherence ceiling (not a fixable estimator bug)

Follow-up to PR #57's probe-level-accuracy diagnosis, which left one question open: is
`coppa_ml::channel_capacity`/`channel_selectivity`'s weak same-frame correlation with the oracle
(0.241 at a level-1 probe) a noisy-but-fixable ESTIMATOR problem, or close to the true information
ceiling a frame-averaged metric can carry? Two new diagnostics answer this directly.

### `capacity_snr_reference_diagnosis.rs`: a specific hypothesis tested and disconfirmed

Hypothesis: every existing Watterson bench builds its received signal as
`coppa_channel::awgn_seeded(&faded, snr_db, seed)`, referencing the injected noise power to the
FADED signal's OWN realized power (`awgn_with_rng`'s `signal_power` is computed from whichever
slice is passed in) rather than to a fixed clean-signal reference — contrary to `watterson.rs`'s own
module doc ("Set noise from the CLEAN signal power ... never from the faded output, or fading
cannot cost SNR"). Prediction: under this self-referenced convention, a frame's average per-carrier
SNR is pinned near the nominal `snr_db` regardless of that trial's real fade depth, so
`corr(fade_ratio, capacity)` should sit near 0 and `corr(capacity, selectivity)` should sit near -1
(one degenerate axis). A clean-referenced control (`awgn_ref_seeded`) should behave oppositely.

`cargo run --release -p coppa-bench --example capacity_snr_reference_diagnosis` (level 1, robust
profile, `snr_db=24`, `TRIALS=400`):

| preset   | convention        | corr(fade_ratio, capacity) | corr(capacity, selectivity) |
|----------|--------------------|----------------------------|------------------------------|
| Poor     | self-referenced   | -0.052                     | 0.878                        |
| Poor     | clean-referenced  | -0.021                     | 0.893                        |
| Moderate | self-referenced   | -0.268                     | 0.734                        |
| Moderate | clean-referenced  | -0.206                     | 0.713                        |

Both conventions behave alike, and `corr(capacity, selectivity)` is strongly *positive* rather than
the predicted -1 — the hypothesis is disconfirmed. The noise-injection convention every existing
Watterson bench uses is not, on this evidence, hiding a frame's overall fade depth from the metric.

### `capacity_ground_truth_diagnosis.rs`: decisive — the estimator is uncorrelated with the real channel

This diagnostic compares `channel_capacity`/`channel_selectivity` (from the real receiver's
pilot-based `nv`) against GROUND TRUTH computed directly from the Watterson channel model's own
per-tap fading gains — not from decode success. `coppa_channel::watterson::watterson_with_gains`
(new, additive; `watterson()` now delegates to it) exposes each tap's raw complex gain array `g(t)`,
letting the diagnostic evaluate `H(f_k, t) = sum_taps sqrt(tap.power) * g_tap(t) *
exp(-i·2π·f_k·tap.delay_s)` at every active OFDM subcarrier frequency, power-averaged over many
time samples spanning the frame. The absolute noise-variance scale is calibrated against the real
receiver's own measured unit-gain (no-fading) `nv` — not a theoretical guess — removing any
question about FFT processing gain or guard-band overhead.

`cargo run --release -p coppa-bench --example capacity_ground_truth_diagnosis` (level 1, robust
profile, `snr_db=24`, `TRIALS=300`):

| preset   | measured cap. (mean/std) | ground-truth cap. (mean/std) | corr(cap, gt cap) | corr(sel, gt sel) | gt stability (full vs. 2nd half) |
|----------|--------------------------|-------------------------------|--------------------|---------------------|-------------------------------------|
| Good     | 2.186 / 1.612            | 7.491 / 1.141                 | 0.050              | 0.131               | 1.000                                |
| Poor     | 1.703 / 1.777            | 8.077 / 0.604                 | -0.019             | 0.036               | 0.812                                |
| Moderate | 1.677 / 1.510            | 7.724 / 0.914                 | -0.224             | -0.144              | 0.953                                |

The ground-truth signal itself is stable within-frame (0.812-1.000 across two disjoint time
windows, confirming the long-coherence-time assumption at the ground-truth level), yet the
receiver's own measured capacity/selectivity are statistically indistinguishable from zero
correlation with it — on every preset, including the mildest (Good: 0.5 ms delay, 0.1 Hz Doppler).
The ~6 bits/s/Hz mean gap and near-zero correlation are both roughly *preset-invariant*: the
estimator isn't tracking fade severity so much as reading a similarly-degraded value regardless of
it, whenever any real multipath is present.

### Conclusion: reinforces the already-closed coarse-delay-drift finding, not a new bug

This is the same conclusion the "Coarse-delay drift Kalman tracker" investigation above reached via
FER outcomes (genuine Rayleigh coherence time shorter than one frame, not a fixable per-frame
estimator bug) — now confirmed independently, via a completely different and decode-independent
method, specifically for the capacity metric RateLoop depends on. Per that investigation's own
conclusion, this is **not reopened** as a new measurement-layer bug: the untried candidate levers
remain coherence-time/airtime reduction (e.g. shorter frames via `hf_standard_short_cp`) or
fade-diversity interleaving, not further estimator tuning. No production code changed as a result of
this diagnosis (`watterson_with_gains` is a new, additive accessor only); both example files are
diagnostics.

## 2026-07-26 — SPEED_LEVEL_MIN_CAPACITY recalibration from clean (post-HARQ-fix) data

PR #61/#62 fixed a bench-harness bug where `coppa-bench`'s trial runners reused one
`CoppaTransceiver`/`seq_num` across many `receive()` calls sharing an IR-HARQ accumulator that
only evicts on success, so an early genuine low-SNR failure could poison every later, unrelated
trial at the same seq. PR #62's commit message flagged one piece of explicit follow-up: whether
`SPEED_LEVEL_MIN_CAPACITY` (`crates/coppa-ml/src/mcs.rs`), originally calibrated from
`mcs_calibration.rs` output produced *before* that fix, is systematically off. This section is
that recalibration, done for real.

Note `mcs_calibration.rs`'s `sound()` function (which reads `channel_capacity`/`channel_selectivity`)
never went through the shared-accumulator `receive()` path in the first place, so the raw C
readings themselves aren't expected to change from the fix. What the fix affects is `fer()` — every
per-level FER reading used to choose which level is goodput-optimal at a given C — and `mcs_calibration.rs`
now unconditionally calls `tx.harq_evict(0)` after every trial (see the file's own comment
referencing `runner.rs`'s regression test), so today's FER readings are clean.

### Step 1-2: calibration grid + derivation

`cargo run -p coppa-bench --release --example mcs_calibration 0xCA11B` (robust profile, 8-frame
averaged sounding, `TRIALS=40`/level/cell, 4 channels × 5 SNRs × 9 levels = 180 `DATA` rows).
Goodput proxy = `eta(level) * (1 - fer)` using `SPEED_LEVEL_EFFICIENCY`. For each `(channel, snr)`
grid point, the argmax level and its goodput:

| channel  | snr | C    | sel  | argmax level | goodput |
|----------|-----|------|------|--------------|---------|
| AWGN     | 6   | 4.53 | 0.20 | **L7**       | 3.000 (FER=0.00 for L1-L7 already; only L9/L10 not yet converged) |
| AWGN     | 12  | 6.43 | 0.19 | **L9**       | 4.000 (FER=0.00) |
| AWGN     | 18  | 7.64 | 0.19 | **L10**      | 5.119 (FER=0.025) |
| AWGN     | 24  | 8.18 | 0.19 | L10          | 4.856 |
| AWGN     | 30  | 8.36 | 0.19 | L10          | 4.856 |
| Good     | 6   | 1.82 | 0.42 | **L4**       | 1.050 |
| Good     | 12  | 2.11 | 0.45 | L4           | 1.050 |
| Good     | 18  | 2.22 | 0.46 | L4           | 1.087 |
| Good     | 24  | 2.24 | 0.46 | L4           | 1.087 |
| Good     | 30  | 2.25 | 0.46 | L4           | 1.087 |
| Moderate | 6   | 1.35 | 0.25 | L4           | 0.788 |
| Moderate | 12  | 1.68 | 0.33 | L6 (0.900) vs L4 (0.870) — 3-pt margin, within 40-trial noise | |
| Moderate | 18  | 1.91 | 0.40 | L4           | 0.938 |
| Moderate | 24  | 1.94 | 0.40 | L4 (0.930) vs L6 (0.900) — flips to L6 at 30 dB, same C | |
| Moderate | 30  | 1.94 | 0.40 | L6 (1.000) vs L4 (0.930) — the flip referenced above | |
| Poor     | *   | 1.26-2.32 | 0.47-0.76 | L6 @ 1.26, then L7 from 1.92 up | excluded from anchoring (outage-floor channel, same precedent as the original table) |

**Anchors used** (lowest tested C where a level is the clean, unambiguous argmax in AWGN, then
Good — Moderate/Poor are informative but not anchor sources, same convention the original table's
own doc comment used):

- **L7 @ C=4.53** (AWGN 6 dB) — AWGN's waterfall is steep enough that L1-L7 already read FER=0.00
  at the *lowest tested* AWGN grid point, so the highest-eta among them (L7, eta=3.0) wins
  outright; L9 (eta=4.0, FER=0.325 there) and L10 (FER=1.0) aren't yet converged enough to compete.
- **L9 @ C=6.43** (AWGN 12 dB) — first point where L9 itself reads FER=0.00.
- **L10 @ C=7.64** (AWGN 18 dB) — first point where L10's goodput (5.119, at FER=0.025) exceeds
  L9's (4.000, FER=0.00).
- **L4 @ C=1.82** (Good 6 dB) — L4 is the *only* level that is ever goodput-optimal anywhere across
  Good's whole tested 6-30 dB range; there is no C in the tested Good sweep where anything else wins.

**L2/L3/L5/L6 have no unambiguous single-level win anywhere in the tested clean-region grid**: L2
and L3 never win in *any* tested (channel, SNR) cell at all (not even in Poor's lowest-C point);
L5 never wins anywhere; L6's apparent wins in Moderate (C=1.68 and C=1.94) are within sampling
noise of a tie with L4 — at C=1.94 specifically, the argmax literally flips between the 24 dB and
30 dB grid points (a 2-percentage-point FER wobble, well inside a 40-trial Bernoulli confidence
interval) despite testing essentially the same C, the signature of noise rather than a real
transition. L6's one clean win is in Poor at C=1.26, and Poor is the known outage-floor channel
excluded from anchoring (same precedent the *original* table's own doc comment used — it never
cited Poor as an anchor source either).

Per the brief's "conservative in an ambiguous/untested region" rule (same principle the original
table used to resolve its AWGN-vs-Good C≈5.9-6.4 overlap), these four un-anchored levels
(L2, L3, L5, L6) are **interpolated**, not directly anchored, by preserving the *previous* table's
relative fractional position within each newly-anchored bracketing interval:

- L2, L3 interpolated between L1=0.0 and the new L4=1.82, using the same fraction the old table's
  L2=1.5, L3=2.2 held within the old [L1=0.0, L4=2.6] interval (0.577, 0.846) → L2=1.05→1.1,
  L3=1.54→1.5.
- L5, L6 interpolated between the new L4=1.82 and new L7=4.53, using the same fraction the old
  table's L5=3.0, L6=4.0 held within the old [L4=2.6, L7=6.5] interval (0.103, 0.359) →
  L5=2.10→2.1, L6=2.79→2.8.

Sanity-checked against `select_speed_level_calibrated_is_monotonic` (table index non-decreasing
as C grows) by eye and by the test itself — passes.

### Step 3: old vs. new table

| level | old `C_min` | new `C_min` | anchor type |
|-------|-------------|-------------|-------------|
| 1     | 0.0         | 0.0         | baseline convention (unchanged) |
| 2     | 1.5         | 1.1         | interpolated |
| 3     | 2.2         | 1.5         | interpolated |
| 4     | 2.6         | **1.8**     | **data-anchored** (Good 6 dB) |
| 5     | 3.0         | 2.1         | interpolated |
| 6     | 4.0         | 2.8         | interpolated |
| 7     | 6.5         | **4.5**     | **data-anchored** (AWGN 6 dB) |
| 9     | 7.2         | **6.4**     | **data-anchored** (AWGN 12 dB) |
| 10    | 8.0         | **7.6**     | **data-anchored** (AWGN 18 dB) |

The whole ladder compressed downward — every threshold dropped, most sharply at the top
(L7 -2.0, L9 -0.8, L10 -0.4) — consistent with the pre-fix table having been calibrated against
FER readings that were, at least at some grid points, corrupted upward by stale IR-HARQ carryover
from an earlier failing trial at the same seq (the exact mechanism PR #61/#62 fixed elsewhere in
the bench suite). Not isolated as the sole cause, in the same spirit as the `milstd`/`session`
re-baseline above: this fresh `mcs_calibration` run also went through the default `receive()`
decode path carrying PR #42's separate `TrackedTaps::equalize` LLR/noise-floor fix, so some of the
downward shift may be attributable to that change too, not only to the HARQ fix. The two effects
were not disentangled here.

### Step 4: dependent unit tests

`select_speed_level_calibrated_uses_thresholds` and `select_speed_level_2d_separates_overlap`
(`crates/coppa-ml/src/mcs.rs`) had their hardcoded `C`/expected-level literals retargeted to the
new table (same assertion *shape* — boundary + conservative-region + overlap-resolution — just
new numbers). `cargo test -p coppa-ml --lib mcs::` — 15/15 pass.

### Step 5: held-out-seed validation (`mcs_compare`, seed 0x5A1AD)

| selector       | pre-fix baseline (PR #62) | recalibrated |
|----------------|---------------------------|--------------|
| calibrated(C)  | 0.741                     | **0.859**    |
| 2D(C, sel)     | 0.828                     | **0.892**    |

Both aggregate ratios (adaptive-selector goodput ÷ per-cell oracle goodput, held-out seed) improve
meaningfully — the recalibration is a real, unambiguous win on this metric.

### Step 6: `closed_loop_arq` — the real point of this task

```
adaptive throughput : 326680 bits
best fixed (L4)     : 357424 bits
per-frame oracle    : 428936 bits
adaptive/oracle = 0.762   adaptive/best-fixed = 0.914
```

Compared against the current committed baseline (0.931/0.775, from PR #58's active overshoot
probing) and the plan's acceptance bar (`adaptive/best-fixed > 1.0`, `adaptive/oracle >= 0.8`):

- **adaptive/best-fixed: 0.931 → 0.914** (slightly worse)
- **adaptive/oracle: 0.775 → 0.762** (slightly worse)
- **The acceptance bar is still not met.** Both ratios move in the wrong direction, though only
  marginally (within the kind of run-to-run noise band this bench has shown at other points in its
  history) — this is **not** the "recalibration closes the gap" outcome, and is reported plainly
  rather than adjusted or buried. `mcs_compare`'s held-out-seed win (Step 5) does not transfer into
  a `closed_loop_arq` win; the two benches are measuring different things (a static-oracle-vs-selector
  ratio at fixed channel snapshots, vs. a live closed-loop hysteresis-and-probing system's aggregate
  throughput over a full non-stationary SNR trajectory), and this result shows they can move in
  different directions from the same underlying table change.

**Conclusion:** `SPEED_LEVEL_MIN_CAPACITY`'s recalibration from clean, HARQ-fix-correct data was
real, necessary follow-up work (the old table's anchors were partly derived from corrupted FER
readings, and the new table's anchors are meaningfully different — every threshold dropped), and it
measurably improves the static `mcs_compare` selector-accuracy metric. It does **not**, however,
move `RateLoop`'s `closed_loop_arq` acceptance-bar shortfall — the gap there remains open, for the
same reasons already tracked in docs/LIMITATIONS.md's RateLoop bullet (the raw per-frame recommendation
signal's volatility during fast fading, not the calibration table's own accuracy, is still believed
to be the dominant remaining factor). See docs/LIMITATIONS.md's RateLoop Known Limitations bullet for the
updated framing.

## 2026-07 — RateLoop capacity-metric level-bias diagnosis

docs/LIMITATIONS.md's `RateLoop` known-limitation bullet root-causes its unmet acceptance bar to
`coppa_ml::channel_capacity` possibly "not being invariant to which speed level a measured frame
happened to use" — a hypothesis that was, until now, an informal, uncommitted diagnostic. This
entry is that diagnostic, committed and run for real:
`cargo run -p coppa-bench --release --example capacity_metric_level_bias` sounds *every* real
speed level (1-7, 9-10 — level 8 is not a defined mode) — not just `mcs_calibration.rs`'s fixed
level-2 probe — against AWGN and Watterson Good/Moderate/Poor, at 5 SNR points (6/12/18/24/30 dB),
40 trials per (channel, SNR, level) cell, seed `0xCA11B`, and reports whether mean
`channel_capacity` trends with level beyond each cell's own 2-standard-error noise bound. All
levels/channels sound against the same fixed `robust` profile (matching `mcs_calibration.rs`'s own
convention, and the profile `SPEED_LEVEL_MIN_CAPACITY`'s calibration was itself derived under) so
that level, not per-level profile choice, is the only thing varying — the live daemon instead
routes levels 1-4 through `hf_standard` and 5+ through `vhf_wide`, so this measures the level
effect in isolation, not the exact live per-level path.

**Real SUMMARY output:**

```
SUMMARY (per channel, averaged across SNR grid):
  AWGN: level1->level10 mean capacity gap = +1.581 bits/s/Hz (2*SE bound = 0.121) -> LIKELY REAL TREND
  Good: level1->level10 mean capacity gap = +0.051 bits/s/Hz (2*SE bound = 1.013) -> within noise
  Moderate: level1->level10 mean capacity gap = +0.485 bits/s/Hz (2*SE bound = 0.876) -> within noise
  Poor: level1->level10 mean capacity gap = +0.540 bits/s/Hz (2*SE bound = 1.175) -> within noise
```

**Interpretation.** AWGN — the negative control, with no fading drift and therefore no reason for
duration/coherence-time effects to matter — is the *one* channel that shows a level-dependent
trend clearing its own noise bound (+1.581 bits/s/Hz, over 13x the 0.121 bound), while all three
Watterson channels (Good, Moderate, Poor) show gaps well within their (much larger, fading-driven)
noise bounds. Per the design doc's "AWGN as negative control is load-bearing" open question, this
result points **away** from the coherence-time/fading-duration mechanism the existing docs/LIMITATIONS.md
hypothesis was built around, not toward it — a real, reproducible level-dependence exists, but it
is present (and clearest) on the one channel where fading-coherence effects cannot be the cause.
The per-level DATA rows show the AWGN capacity reading climbing fairly monotonically with level at
every SNR (e.g. at 24 dB: level 1 = 8.412, level 5 = 10.567, level 10 = 10.628 bits/s/Hz), which is
consistent with something structural to how `channel_capacity` is computed per level (e.g. pilot
density/placement, equalizer behavior, or noise-variance estimation differing systematically by
modulation/constellation order) rather than anything fading-related. This is a real, useful, but
different finding than the one docs/LIMITATIONS.md's existing bullet assumed: it confirms `channel_capacity`
is level-dependent on the *same underlying channel* (the core suspicion), but redirects the likely
mechanism away from fading-coherence time and toward something in the AWGN-reproducible per-level
measurement path itself. Distinguishing which specific stage of that path (pilot extraction,
equalization, or the noise-variance estimator feeding `channel_capacity`) is responsible is out of
scope for this diagnose-only task and is the natural next investigation. Note the Watterson
channels' "within noise" verdict is an underpowered negative, not evidence the bias is absent
under fading: all three fading channels show the same positive-signed gap AWGN does (Good +0.051,
Moderate +0.485, Poor +0.540), just too small relative to fading's much larger per-trial capacity
variance for 40 trials to resolve — a real possibility worth keeping in mind for the next
investigation is that the same mechanism is present under fading too, just buried in that noise.

## 2026-07 — RateLoop capacity-metric level-bias: root cause isolated to per-level PAPR clip targets

Follow-up to the section directly above, which redirected suspicion for the AWGN-only
`channel_capacity` level-dependence away from fading-coherence time and toward "something in the
per-level measurement path itself," naming pilot density, equalizer path, and clip-induced
noise-variance bias as candidates. This entry runs that investigation down.

**Hypothesis.** `SPEED_LEVELS[level].papr_target_db` is a *deliberately* level-dependent hard-clip
threshold (6.0 dB at BPSK up to 14.0 dB at 64-QAM — see `docs/LIMITATIONS.md`'s PAPR bullet) applied once to
the whole frame's time-domain samples in `CoppaModem::modulate_mapped`'s TX-conditioning chain
(`papr_clip`, after RC-overlap windowing, before the optional HF bandpass). This clip acts on
*every* sample in the frame — preamble, the full-comb probe symbol, the header, and every payload
OFDM symbol's pilots — at one shared per-frame threshold (`rms * 10^(target_db/20)`). A tighter
threshold (closer to RMS) means more samples get clipped, which distorts the probe/pilot
subcarriers that `probe_calibration` and the per-symbol Kalman tracker read their noise variance
from. Since low levels (BPSK) get the harshest clip (6.0 dB) and high levels (64-QAM) get the
loosest (14.0 dB), this predicts an SNR-independent, level-dependent noise floor baked directly
into the pilot channel estimate — worse (higher apparent noise, lower capacity) at low levels,
better at high levels — that only becomes visible once thermal AWGN shrinks enough to stop masking
it (exactly the high-SNR regime where the prior section's trend was strongest).

**Falsifiable prediction.** If this is the mechanism, forcing every level through the *same* clip
target — holding modulation order, pilot pattern, symbol content, and everything else exactly as
production uses them — should collapse the level1→level10 gap in **both** directions: giving low
levels the loose 14.0 dB target only level 10 normally gets should raise their capacity; forcing
level 10 through the harsh 6.0 dB target only levels 1/2 normally get should lower its capacity. If
the gap survives a uniform clip target similarly to how it appears naturally, that falsifies PAPR
clipping as the mechanism.

**Method.** New `crates/coppa-bench/diagnostics/papr_clip_level_bias.rs`: for each of the 9 real
speed levels, builds ONE fixed set of constellation-mapped payload symbols (pseudorandom bits
through that level's real mapper, `CODED_BLOCK_LEN.div_ceil(bits_per_symbol)` symbols — the same
count `CoppaTransceiver::transmit` produces for one codeword) and modulates it three ways, varying
*only* `papr_target_db`: `natural` (this level's real `SPEED_LEVELS` target — reproduces what
production ships), `uniform_loose` (every level forced through 14.0 dB, level 10's real target),
and `uniform_harsh` (every level forced through 6.0 dB, level 1/2's real target). Each signal is
AWGN-faded 40 times per SNR (6/12/18/24/30 dB, seed `0xCA11B` — same grid, trial count, and
significance-check convention as the prior section's bench) and demodulated via the real
`CoppaModem::demodulate_frame`.

**Real SUMMARY output:**

```
SUMMARY (per condition, averaged across SNR grid):
  natural: level1->level10 mean capacity gap = +1.554 bits/s/Hz (2*SE bound = 0.125) -> LIKELY REAL TREND
  uniform_loose: level1->level10 mean capacity gap = -0.117 bits/s/Hz (2*SE bound = 0.134) -> within noise
  uniform_harsh: level1->level10 mean capacity gap = -0.061 bits/s/Hz (2*SE bound = 0.119) -> within noise
```

**Interpretation.** `natural`'s +1.554 bits/s/Hz gap (2×SE bound 0.125) closely reproduces PR #51's
independently-measured +1.581 (bound 0.121) using a completely different symbol-construction path
(pseudorandom mapper output here vs. real LDPC-encoded payloads there) — confirming the effect
isn't an artifact of either bench's specific payload content. Both uniform conditions collapse the
gap to **within noise, in both directions**: giving every level the loose target does not leave a
residual climb (it's slightly negative, −0.117), and giving every level the harsh target does not
leave a residual climb either (−0.061) — if the bias were coming from somewhere else in the
per-level measurement path (e.g. pilot density or an equalizer/noise-estimator effect intrinsic to
modulation order) it should have survived at least partially in these matched-clip conditions,
since only the clip target was pinned equal, not that path. It didn't: at 30 dB, `natural` spans
8.560 (level 1) to 11.451 (level 10) — a 2.9 bit/s/Hz range — while `uniform_harsh` at the same SNR
spans only 8.450–8.625 (0.18 bit/s/Hz) and `uniform_loose` spans 11.392–11.720 (0.33 bit/s/Hz),
both roughly an order of magnitude tighter. **This closes the "which stage" question the prior
section left open: the per-level `papr_target_db` schedule, not pilot extraction, equalization, or
a modulation-order-intrinsic noise-variance effect, is the dominant driver of the AWGN
level-dependence.**

A small residual is worth flagging honestly rather than glossing over: within both uniform
conditions, level 4 (QPSK 3/4) reads consistently highest and level 9 (64-QAM 2/3) consistently
lowest of the 9 levels (e.g. `uniform_harsh`/30 dB: level 4 = 8.625 vs. level 9 = 8.450, a 0.175
spread) — small, well inside each condition's own within-level trial noise at the endpoints used
for the headline gap, but a consistent enough ordering across both uniform conditions that it may
be a second, much smaller effect (plausibly the fixed pseudorandom bit-content differing by level,
producing a slightly different composite time-domain peak profile before the shared clip threshold
applies) — not investigated further here, and irrelevant to the headline finding since it is roughly
an order of magnitude below the `natural` gap it was meant to explain.

**Not fixed — this is the honest, intentional design tension underneath the bias.** The per-level
PAPR clip schedule is not a bug: BPSK's tighter headroom vs. 64-QAM's looser one is a deliberate,
already-tuned real-hardware TX engineering tradeoff (see the PAPR bullet's history), and equalizing
it across levels to remove this bias would cost real EVM/goodput at the levels it's currently tuned
for — not a "trivial, obviously-safe one-line fix" per this task's own bar for when to patch rather
than only diagnose. The actual gap is one layer up: `coppa_ml::channel_capacity` and
`recommend_speed_level` treat a measured capacity reading as if it were a level-independent
"channel truth," when in fact it is measured through a per-level self-noise floor the TX itself
deliberately introduces and that varies by level in a fully known, deterministic way (the
`SPEED_LEVELS` table). A future fix would need to either calibrate `SPEED_LEVEL_MIN_CAPACITY`'s
per-level thresholds to already account for each level's own clip-induced floor (closer to what the
existing calibration may already partially absorb, since it was sound per-level, not
cross-level-compared) or make the closed-loop recommender's *cross-level* comparison
clip-floor-aware before trusting a raw capacity reading taken at one level to predict performance at
another. Left open for the next pass, per this task's diagnose-only scope; `RateLoop`,
`coppa_ml::mcs`, and the `SPEED_LEVELS` PAPR table are unchanged by this investigation.

Reproduce: `cargo run -p coppa-bench --release --example papr_clip_level_bias` (a few seconds
wall-clock — 9 levels × 3 conditions × 5 SNRs × 40 trials of the modem's own OFDM modulate/AWGN/
demodulate path, no fading).

## 2026-07 — RateLoop capacity/selectivity level-bias correction

Follow-up to the two sections directly above (PR #51 confirmed the AWGN level-dependence; PR #52
root-caused it to `SPEED_LEVELS[level].papr_target_db`, both diagnose-only). This entry lands the
fix: `coppa_ml::recommend_speed_level` now takes the frame's originating speed level and corrects
its raw `channel_capacity`/`channel_selectivity` readings for that level's known PAPR-clip-induced
self-noise floor before the calibrated lookup, using a `(level × SNR)` correction table measured by
the new `crates/coppa-bench/diagnostics/capacity_level_correction_calibration.rs` (profile `robust`,
seed `0xCA11B`, 200 trials/cell, AWGN only). The bias turned out not to be a flat per-level offset —
it grows with SNR (e.g. level 10's correction is +0.10 bits/s/Hz at 6 dB but +3.30 at 30 dB) — so
the correction interpolates over the same 5-point SNR grid the diagnostic benches used, on the
frame's own already-computed SNR estimate. `channel_selectivity` is separately level-biased in the
same compounding direction (lower apparent selectivity at higher levels further inflates
`select_speed_level_2d`'s effective capacity) and is corrected the same way. Both correction tables
are reproduced in full in `crates/coppa-ml/src/mcs.rs`'s `CAPACITY_LEVEL_CORRECTION`/
`SELECTIVITY_LEVEL_CORRECTION` consts.

**Scope, unchanged from the design doc**: `SPEED_LEVELS[level].papr_target_db` and all TX
conditioning code are untouched — the per-level clip schedule remains an intentional, already-tuned
tradeoff. The correction is derived entirely from AWGN measurements (the one channel PR #51 confirmed
the bias on) and applied universally regardless of actual channel type, since the receiver has no
ground truth about which channel produced a given frame — a known, explicitly-accepted
approximation for the Watterson case.

**Real re-measured `closed_loop_arq` numbers** (same bench, same acceptance bar as before —
adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8):

```
=== Closed-loop adaptive rate (300 frames, robust profile) ===
adaptive throughput : 286288 bits
best fixed (L4)     : 357424 bits
per-frame oracle    : 428936 bits
adaptive/oracle = 0.667   adaptive/best-fixed = 0.801
```

These numbers are *worse* than the pre-fix baseline (adaptive/best-fixed 0.894→0.801,
adaptive/oracle 0.751→0.667), not better, and the `>1.0`/`≥0.8` bar remains unmet — reported
honestly, per the prior agreement that clearing it is not a condition of landing this PR. The
bench's own level-vs-channel trace narrows down where the regression concentrates: the first two
schedule thirds (the pure-AWGN SNR ramp 3→30→3 dB) track the channel the same way as before, but the
final third (Watterson Good then Poor, both pinned at a nominal 24 dB) is where the loop now drops to
L2 and then L1 and stays there despite the nominal SNR holding flat at 24 dB the whole time — visibly
worse than "slow to climb back," an active, sustained drop. This is consistent with the design doc's
explicitly flagged, known approximation: the correction table is derived exclusively from AWGN
measurements and applied unconditionally to every frame regardless of actual channel type, since the
receiver has no ground truth about which channel produced a given frame. Watterson fading's real
per-carrier noise-variance profile (deep frequency-selective fades, not AWGN's flat floor) is not the
regime the correction was calibrated against, and applying an AWGN-derived, SNR-keyed correction to a
Watterson-faded reading during this bench's fading tail appears to over-subtract capacity at exactly
the levels/SNRs where the loop had climbed high beforehand — actively hurting performance in the one
regime the design doc called out up front as "a reasonable extrapolation, not a validated one." This
is a real, measured regression against the specific pre-fix numbers CLAUDE.md and the "Phase 3 Task 4"
section above recorded, concentrated in the Watterson-fading portion of this particular bench
schedule, not a general worsening of the AWGN-only case the correction was built and validated
against.

**Correction on the "no ground truth" framing above, and a concrete next step.** A final
whole-branch review pointed out that "the receiver has no ground truth about which channel produced
a given frame" overstates the case: `channel_selectivity(noise_vars)` is already computed on the
very next line inside `recommend_speed_level` and IS a cheap, already-available discriminator
(near-zero on AWGN's flat floor, substantially higher under frequency-selective Watterson fading) —
it just isn't used to gate or scale the correction today. The most promising untried lever for
closing this Watterson-tail regression is a selectivity-scaled correction: attenuate the AWGN-derived
correction as raw selectivity rises above the AWGN floor, rather than gating it to zero on fading,
since the underlying PAPR-clip self-noise bias is a TX-side effect present on every channel type,
not an AWGN-specific artifact — a hard on/off gate would under-correct the real bias on fading, not
just avoid over-correcting it. Not attempted here; left as the next concrete step for whoever picks
this up.

No regression observed on `tests/phase_c_loopback.rs`'s full-ladder AWGN/Watterson FER sweep (Step
2 above) — the correction, applied only inside `recommend_speed_level`, does not affect
`channel_capacity`/`channel_selectivity`'s other callers or the LDPC-facing noise-variance path.
`test_snr_fer_monte_carlo` was re-run in full and still shows FER=0.00/100 for every level (1, 3-7,
9-10) across its whole swept SNR range, unchanged from CLAUDE.md's documented baseline.
(COP-70 correction: that sweep could not fail, since its full-band noise was ~9 dB optimistic
against the 3 kHz convention, its window sat above every level's waterfall, and it asserted
nothing, so this sentence is not evidence of the absence of a regression.)

Reproduce: `cargo run -p coppa-bench --release --example capacity_level_correction_calibration`
(calibration table) and `cargo run -p coppa-bench --release --example closed_loop_arq` (impact
measurement).

