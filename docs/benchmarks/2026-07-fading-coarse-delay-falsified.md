> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Fading root-cause investigation: the coarse-delay-reference theory falsified

The Cascaded design (below) and the earlier Replace design (PR #41) both measured Watterson-Moderate/level 2 FER≤10% at exactly **18.0 dB** — identical despite different downstream mechanisms (fresh per-window least-squares refits vs. an AR(1) Kalman tap tracker) consuming the same `DriftTracker` per-window coarse-delay estimate. That coincidence motivated two parallel diagnostic investigations, run down independently, both landing on the same conclusion: the "accumulating phase/coarse-delay reference error, not stationary Rayleigh amplitude fading" theory that motivated all four attempts (Task 1, Task 7, Replace, Cascade) is very likely **backwards**.

### Investigation 1: is `DriftTracker`'s own estimate the bottleneck?

New `CoppaModem::diagnose_drift_tracking` hook + `crates/coppa-bench/examples/drift_estimate_quality_diagnosis.rs` (150 trials/point, real frames, AWGN control vs. Watterson-Moderate/level 2):

| | AWGN, 12→24 dB | Watterson-Moderate, 12→24 dB |
|---|---|---|
| within-frame tracked-`τ` range (grid units) | 0.060 → 0.018 (shrinks with SNR, as expected) | 0.432 → 0.537 (flat/rising — does NOT shrink) |
| `τ` vs. frame-fixed `coarse_delay` | 0.033 → 0.008 | 0.162 → 0.235 |
| hard-decision BER | 0.000 everywhere | 0.085–0.090, never clears |

The AWGN control behaves exactly as a well-tuned estimator should. On Watterson-Moderate the tracked delay is 10–30× noisier and **doesn't converge even at 24 dB**, where thermal noise is negligible — no amount of Kalman-filter retuning fixes an estimator chasing a target that itself won't hold still. The magnitude (0.16–0.24 grid units, frequently exceeding the `COARSE_DELAY_JITTER_BOUND=0.15` threshold already known from the CFO×level-4 work to start degrading Watterson survival) is consistent with the channel's own two independently-fading taps (separated ≈2.4 grid units) trading dominance — the "coarse delay" being tracked is a power-weighted average of two moving targets, not a single slowly-drifting reference. Per-frame `τ`-volatility does not predict that frame's own decode outcome (`corr_tau_range_vs_ber ≈ 0` at every SNR), consistent with the swinging being a near-universal property of the channel rather than an occasional-bad-frame effect.

### Investigation 2: what's actually causing the frame loss?

New `crates/coppa-bench/examples/regression_root_cause.rs` (400 trials/point, real Watterson-Moderate/level 2, 9–24 dB):

1. **The codebase's own "amplitude-fading coherence time ~1-10s, much longer than one frame" assumption (`kalman_tracker.rs`, carried through Task 1/7's reasoning) was never checked against `coppa-channel`'s actually-configured Watterson-Moderate Doppler spread.** `WattersonPreset::Moderate` sets `doppler_spread_hz = 0.5` → PSD sigma `0.25` Hz (the assumption text also conflated the *spread* with the *sigma*, a related unit slip). Plugging the correct sigma into `watterson.rs`'s own verified `rho(τ)=exp(-2π²σ²τ²)` autocorrelation: a level-2 (`hf_standard`, 1.365 s) frame decorrelates to **~10% correlation by frame-end** — the channel loses coherence *within* a single frame. (Good, at σ=0.05 Hz, genuinely fits the original "much longer than a frame" claim — ρ stays above 91% for the whole frame; the assumption was never wrong for Good, only silently over-generalized to Moderate/Poor.)
2. **`LdpcNotConverged` failure counts are SNR-independent** across 9→24 dB (14/16/13/15/12/13, a 1.08× ratio end-to-end) while `SyncFailed` is clearly SNR-driven (46→2, 23.0×) — the signature of a hard, channel-realization-dependent fading-outage floor, not an estimation-quality problem that should shrink with more SNR margin.
3. **A flat single-tap channel** (no multipath at all — "stale coarse-delay reference" isn't even a coherent failure mode there) at Moderate's real Doppler already produces **23.25%/12.75%/3.50% FER at 9/12/15 dB** — comparable to or *worse than* the real 2-tap channel (15.00%/10.75%/5.25%) at the same points. Pure Rayleigh amplitude-fade cost, with zero multipath involved, accounts for most of the AWGN-to-Moderate gap on its own.

### Net conclusion

All four attempts (Task 1's estimator, Task 7's AR(1) tracker, Replace, Cascade) were very likely chasing a *symptom* of genuine Rayleigh amplitude fading — whose real coherence time on Watterson-Moderate is far shorter than this codebase assumed — rather than its cause, which no phase/delay-reference correction can fix. This is consistent with, not contradictory to, the earlier diagnostics' own raw measurements (the within-frame `|H|²` decay Task 1 documented is real) — the *interpretation* of that decay as a fixable reference-staleness problem was the error, not the measurement itself.

**This closes the coarse-delay-reference line of investigation** — no fifth variant is planned. Untried candidate levers for whoever picks this up next: coherence-time/airtime reduction (shorter frames — the existing `hf_standard_short_cp` infrastructure already reduces frame duration and could be evaluated against Watterson-Moderate specifically) or fade-diversity interleaving, rather than further channel-estimation refinement.

Reproduce: `cargo run -p coppa-bench --release --example drift_estimate_quality_diagnosis` / `cargo run -p coppa-bench --release --example regression_root_cause`.

