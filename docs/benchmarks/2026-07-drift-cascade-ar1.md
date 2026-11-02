> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Cascaded coarse-delay drift + AR(1) tap tracker (measured, gate NOT met, shipped disabled — third and final attempt in this line)

`docs/superpowers/specs/2026-07-18-cascaded-drift-tracker-design.md` proposed a third architecture
for the still-open Watterson-Moderate/level 2 coarse-delay-drift regression documented in
docs/LIMITATIONS.md: a Cascaded payload path
(`crate::ofdm::coppa_modem::CoppaModem::equalize_payload_cascaded`, `DRIFT_CASCADE_ENABLED`
toggle) combining `drift_tracker::DriftTracker`'s per-window coarse-delay tracking with the AR(1)
`KalmanLagSmoother` tap-amplitude tracker, instead of `equalize_payload_drift_tracked`'s fresh
per-window `DelayDomainEstimator::fit` refits. Ahead of the Cascade itself, this plan's Task 1
fixed a real, independent LLR-overconfidence bug in `TrackedTaps::equalize` (it was feeding the
AR(1) tracker's shrinking posterior tap variance into the LLR noise denominator instead of the
fixed observation noise `sigma_v2`) — **unconditionally**, so it also recalibrates the
already-shipped `equalize_payload_kalman` default path, not just new code.

**Acceptance bar:** Watterson-Moderate/level 2 FER≤10% ≤16.5 dB, AWGN and Watterson-Good
unregressed. **Not met** — see below. `DRIFT_CASCADE_ENABLED` stays `false`.

### Step 1: baseline (both toggles `false`, Task 1's fix included, 400 trials/point)

| Channel | Level 2 FER≤10% | Level 2 FER≤1% | Level 5 FER≤10% | Level 5 FER≤1% |
|---|---|---|---|---|
| AWGN | 6.0 dB | 9.0 dB | 15.0 dB | 15.0 dB |
| Watterson-Moderate | **15.0 dB** | never | never | never |
| Watterson-Poor | never | never | never | never |

The plan's own text flagged that Task 1's unconditional noise-formula fix might shift this number
away from the 15.0 dB already on record (from the prior "Replace" design's own measurement,
below). It didn't: the real measured baseline on this branch, with Task 1's fix included, is
exactly **15.0 dB** — unchanged. A legitimate, unremarkable outcome, not a bug: the fix corrects
the LLR noise denominator fed into decoding, and evidently doesn't move this particular FER≤10%
threshold at this operating point and trial count. AWGN and Watterson-Poor are also identical to
the prior baseline. Used as this task's reference point, per the plan's own Step 1 instruction.

### Step 2: full gate with the Cascade enabled (`DRIFT_CASCADE_ENABLED = true`, `DRIFT_Q_TAU`/`DRIFT_Q_DOT` unchanged, 400 trials/point)

| Channel | Level 2 FER≤10% | Level 2 FER≤1% | Level 5 FER≤10% | Level 5 FER≤1% |
|---|---|---|---|---|
| AWGN | 6.0 dB (unchanged) | 9.0 dB (unchanged) | 15.0 dB (unchanged) | 15.0 dB (unchanged) |
| Watterson-Moderate | **18.0 dB** | never | never | never |
| Watterson-Poor | never | never | never | never |

### Step 3: decision — bar NOT met, and a measured regression against the current baseline

Watterson-Moderate/level 2 FER≤10% with the Cascade enabled is **18.0 dB**, against a required
**≤16.5 dB** — not met. AWGN is exactly unchanged at both levels (no regression, bar clause
satisfied). Watterson-Poor never clears either way (unaffected). Watterson-Good was not part of
the measured channel set (same convention as `drift_tracker_gate.rs`/`task1_gate.rs`/
`task7_gate.rs`). Against this branch's own current baseline (Step 1, 15.0 dB), enabling the
Cascade is a real **~3 dB regression** — not merely a shortfall against the 16.5 dB target, but
actively worse than shipping nothing. 18.0 dB is, notably, *exactly* the same number the pure
`DriftTracker`-only "Replace" design measured (see the next section below) — combining it with
the AR(1) tap tracker made no measurable difference to this metric, for better or worse.

**Decision: `DRIFT_CASCADE_ENABLED` stays `false`.** This is the third and, per the design plan's
own framing, final attempt at this specific Watterson-Moderate/level 2 regression in this
specific line of investigation: Task 1/7's original delay-domain estimator + AR(1) tap tracker
(regressed 18.0 dB → 24.0 dB → 30.0 dB across two Phase 2 tasks, before turbo re-estimation and
later phases' work brought the shipped cumulative number back down to 15.0 dB), the pure
`DriftTracker` "Replace" design (measured 18.0 dB, a ~3 dB regression against the same 15.0 dB
baseline), and now this Cascaded design (also 18.0 dB — no improvement over either). All three
variants of "track the coarse-delay reference itself as a per-window state" have now
independently landed at the same 18.0 dB point or worse. A fourth variant in this line is not
pre-planned, and the honest conclusion is that per-window coarse-delay tracking, in every form
tried so far, does not close this regression — the root cause may need a genuinely different
hypothesis than "the coarse-delay reference is stale within a frame." The shipped codec's
cumulative Watterson-Moderate/level 2 number (15.0 dB) already clears the 16.5 dB target today
without any of these three tracker variants contributing anything positive to this specific
metric.

Reproduce: `cargo run -p coppa-bench --release --example drift_cascade_gate` (toggle
`DRIFT_CASCADE_ENABLED` in `coppa_modem.rs` to select which path is measured). Full report:
`.superpowers/sdd/task-4-report.md`.

