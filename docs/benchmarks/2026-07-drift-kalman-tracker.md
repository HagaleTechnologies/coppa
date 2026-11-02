> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Coarse-delay drift Kalman tracker (Task 5: measured, gate NOT met, shipped disabled)

`docs/superpowers/specs/2026-07-17-coarse-delay-drift-tracker-design.md` proposed a second,
independent Kalman tracker (`crate::ofdm::drift_tracker::DriftTracker`, 2-state `[τ, τ̇]`
integrated-random-walk model, `coppa_modem.rs`'s `DRIFT_TRACKER_ENABLED` payload-pass toggle)
aimed at the still-open Phase 2 Task 1/7 coarse-delay-reference-drift regression documented in
docs/LIMITATIONS.md (Task 7's AR(1) tap-amplitude Kalman tracker measured
Watterson-Moderate/level 2 FER≤10% at 30.0 dB in isolated testing, worse than both the
pre-Phase-2 18.0 dB baseline and Task 1's own 24.0 dB). This task (Task 5) ran the design's own
gate bench, tuned its two process-noise constants, and made the ship/no-ship call.

**Acceptance bar:** Watterson-Moderate/level 2 FER≤10% ≤16.5 dB, AWGN and Watterson-Good
unregressed. **Not met** — see below. `DRIFT_TRACKER_ENABLED` stays `false`.

### An important, honestly-reported wrinkle: the *current* baseline is not 30.0 dB

Before touching the new tracker, Step 1's sanity check (`DRIFT_TRACKER_ENABLED = false`, i.e.
today's shipped payload path unchanged) was expected, per the design brief, to reproduce Task
7's isolated 30.0 dB figure. It did not — it measured **15.0 dB**, already clearing the ≤16.5 dB
bar and beating the pre-Phase-2 18.0 dB baseline outright. This is not a bug or a silent
regression introduced by this feature's Tasks 1-3 (the new `drift_tracker` module is inert
dead-code weight when the toggle is `false`, and the branch is a compile-time `if
DRIFT_TRACKER_ENABLED {..} else {..}` on the pre-existing `equalize_payload_kalman` path,
confirmed by direct code inspection). It is a real, already-documented consequence of the
several Phase 2/3/4 tasks layered on since Task 7's isolated report: this file's own "Phase 2
... cumulative re-baseline" section above (Watterson Moderate table) already records BPSK 1/2
(level 2) at **15.0 dB**, `Phase 1 exit 18.0 dB → Phase 2 15.0 dB (+3 dB, better)`, attributed
there to turbo re-estimation's benefit on BPSK outweighing the Task 1/7 estimator regression at
that specific level. Task 7's 30.0 dB figure was measured in isolation, before turbo
re-estimation and the rest of Phase 2 Task 8's changes landed; today's real, cumulative number
on this branch is 15.0 dB. Both facts are true and non-contradictory — docs/LIMITATIONS.md's own Known
Limitations already says as much ("levels 1-2 (BPSK) improve at FER≤10% ... outweighing the
estimator regression"). This task proceeded past Step 1 on that basis rather than treating it as
a blocking discrepancy, and measures the new tracker against both the 16.5 dB design bar *and*
this branch's real 15.0 dB current baseline.

### Step 1: baseline sanity check (`DRIFT_TRACKER_ENABLED = false`, full gate, 400 trials/point)

| Channel | Level 2 FER≤10% | Level 2 FER≤1% | Level 5 FER≤10% | Level 5 FER≤1% |
|---|---|---|---|---|
| AWGN | 6.0 dB | 9.0 dB | 15.0 dB | 15.0 dB |
| Watterson-Moderate | **15.0 dB** | never | never | never |
| Watterson-Poor | never | never | never | never |

### Step 2: reduced-scale `(DRIFT_Q_TAU, DRIFT_Q_DOT)` sweep (150 trials/point, watterson-moderate/level 2, 9-27 dB, `DRIFT_TRACKER_ENABLED = true`)

All 9 combinations of `DRIFT_Q_TAU ∈ {1e-5, 1e-4, 1e-3}` × `DRIFT_Q_DOT ∈ {1e-6, 1e-5, 1e-4}`
were run at the reduced scale the design brief specified (not a further-reduced subset — the
full 9-combination grid was completed). Point estimates (percent FER) at each SNR, all 9
combinations essentially indistinguishable within trial noise:

| SNR (dB) | 1e-5/1e-6 | 1e-5/1e-5 | 1e-5/1e-4 | 1e-4/1e-6 | 1e-4/1e-5 | 1e-4/1e-4 | 1e-3/1e-6 | 1e-3/1e-5 | 1e-3/1e-4 |
|---|---|---|---|---|---|---|---|---|---|
| 9  | 20.67 | 21.33 | 21.33 | 21.33 | 21.33 | 20.67 | 21.33 | 20.67 | 20.67 |
| 12 | 13.33 | 13.33 | 13.33 | 13.33 | 13.33 | 13.33 | 13.33 | 13.33 | 13.33 |
| 15 | 14.00 | 14.00 | 14.00 | 14.00 | 14.00 | 14.00 | 14.00 | 14.00 | 14.00 |
| 18 | 10.00 | 10.00 | 10.00 | 10.00 | 10.00 | 10.00 | 10.00 | 10.00 | 10.00 |
| 21 | 5.33  | 5.33  | 5.33  | 5.33  | 5.33  | 5.33  | 5.33  | 5.33  | 5.33  |
| 24 | 5.33  | 6.00  | 5.33  | 5.33  | 5.33  | 5.33  | 5.33  | 5.33  | 5.33  |
| 27 | 6.67  | 6.67  | 6.67  | 6.67  | 6.67  | 6.67  | 6.67  | 6.67  | 6.67  |

None of the 9 combinations clears the FER≤10% Wilson-CI bound anywhere in the swept range at
this trial count (closest: 21/24 dB, point estimate 5.3%, upper-95%-CI bound 0.1017 — just
barely missing 0.10, an almost exact echo of Task 7's own `a`-sweep's closest miss at the same
reduced scale). This flatness across three orders of magnitude of both process-noise constants
mirrors Task 7's own `DEFAULT_SIGMA_D_HZ`-sweep finding: once the basic model is fixed, this
particular tuning axis is not where the remaining gap lives. The plan's original starting point
(`DRIFT_Q_TAU=1e-4`, `DRIFT_Q_DOT=1e-5`) was kept as the shipped (disabled) default — no
combination in the tested range does measurably better.

### Step 3: full gate on the shipped combination (`DRIFT_Q_TAU=1e-4`, `DRIFT_Q_DOT=1e-5`, `DRIFT_TRACKER_ENABLED = true`, 400 trials/point)

| Channel | Level 2 FER≤10% | Level 2 FER≤1% | Level 5 FER≤10% | Level 5 FER≤1% |
|---|---|---|---|---|
| AWGN | 6.0 dB (unchanged) | 9.0 dB (unchanged) | 15.0 dB (unchanged) | 15.0 dB (unchanged) |
| Watterson-Moderate | **18.0 dB** | never | never | never |
| Watterson-Poor | never | never | never | never |

### Step 4: decision — bar NOT met, and a measured regression against the current baseline

Watterson-Moderate/level 2 FER≤10% with the drift tracker enabled is **18.0 dB**, against a
required **≤16.5 dB** — not met. AWGN is exactly unchanged at both levels (no regression, bar
clause satisfied). Watterson-Poor never clears either way (unaffected). Watterson-Good was not
part of the measured channel set (consistent with `task1_gate.rs`/`task7_gate.rs` convention);
a future revisit should add a Good measurement before re-evaluating the bar. But the more important
number is the comparison against this branch's *own current* baseline established in Step 1:
enabling the drift tracker takes Watterson-Moderate/level 2 from **15.0 dB → 18.0 dB**, a real
**~3 dB regression** against what already ships today — not merely "doesn't reach the target,"
but actively worse than doing nothing. (18.0 dB happens to exactly match the old pre-Phase-2
baseline, i.e. the drift tracker's payload path is giving up the turbo-re-estimation-era gain
the existing `equalize_payload_kalman` path currently enjoys, without any offsetting win of its
own.)

**Decision: `DRIFT_TRACKER_ENABLED` stays `false`.** Per the design plan's own instruction for
this outcome, the Cascaded fallback architecture (re-adding Task 7's AR(1) tap-Kalman on top of
drift-corrected windows) was **not** attempted as part of this task — it is a separate
implementation effort needing its own plan/brief. It remains the recommended next step for
whoever picks this up, given the design's own two independent Kalman-based attempts
(Task 7's AR(1) tap tracker, this task's `[τ, τ̇]` random-walk tracker) have now both
independently failed to beat the *estimation-layer* baseline in isolation — though, per the
finding above, the shipped codec's *cumulative* Watterson-Moderate/level 2 number (15.0 dB, from
turbo re-estimation and other Phase 2 Task 8 changes) already clears this task's own 16.5 dB
target today without either Kalman variant contributing anything positive to that specific
metric.

**Update (2026-07-18):** the Cascaded fallback architecture described above was subsequently
attempted and also did not clear the bar — see "Cascaded coarse-delay drift + AR(1) tap tracker"
above, which measured the same 18.0 dB and closes out this line of investigation as the third and
final attempt.

Reproduce: `cargo run -p coppa-bench --release --example drift_tracker_gate` (toggle
`DRIFT_TRACKER_ENABLED` in `coppa_modem.rs` to select which path is measured). Full report:
`.superpowers/sdd/task-5-report.md`.

