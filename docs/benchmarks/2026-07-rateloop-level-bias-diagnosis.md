> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — RateLoop capacity-metric level-bias diagnosis

docs/LIMITATIONS.md's `RateLoop` known-limitation bullet root-causes its unmet acceptance bar to
`coppa_ml::channel_capacity` possibly "not being invariant to which speed level a measured frame
happened to use" — a hypothesis that was, until now, an informal, uncommitted diagnostic (see
`docs/superpowers/specs/2026-07-20-rateloop-capacity-metric-bias-diagnosis-design.md` for the full
design rationale). This entry is that diagnostic, committed and run for real:
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

