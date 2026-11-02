> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

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

