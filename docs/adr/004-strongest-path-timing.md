# ADR-004: Strongest-path (not first-path) sync timing anchor for HF multipath

## Status

Accepted

## Context

Phase 1's Task 5 (`SyncDetector`, see ADR-003 decision 5) replaced the batch `detect_coppa_sync`
search with a streaming detector and, as part of that migration, deliberately changed the
timing-refinement policy: `detect_coppa_sync`'s cross-correlation refinement picked the
**strongest** peak in a `±CP` search window around the coarse Schmidl-Cox estimate (documented in
the Phase 1 plan as "argmax = strongest path — the defect being fixed"). `SyncDetector` instead
picks the **earliest** local peak that clears 50% of the window's global correlation max
(`FIRST_PATH_FRACTION`, `find_first_path`), falling back to the strongest peak only if no earlier
peak clears that threshold. The stated rationale (Phase 1 plan, Task 5) was that anchoring on a
strong, delayed echo risks positioning the FFT window at or near — or, for large enough real
multipath delay, past — the edge of the CP-protected ISI-free region; anchoring on the first
arrival is the textbook-safe choice. An acceptance test
(`detector_locks_first_path_not_strongest`) locked this in with a static two-tap scenario: a
weaker (0.6 amplitude) direct path plus a stronger (1.0 amplitude) echo 96 samples later,
asserting the detector must lock onto the direct path.

Phase 1's own final acceptance sweep (BENCHMARKS.md, "Phase 1 (radio reality): final acceptance
sweep", and ADR-003's Consequences) found a severe, previously-undiscovered regression: the
sparse-pilot HF profiles (`hf_standard`/`hf_wide`/`hf_narrow`, 2-4 pilots) never clear FER≤10% on
Watterson Moderate/Poor anywhere in a -6...30 dB sweep post-Phase-1, versus clearing it at 21 dB
pre-Phase-1. That sweep root-caused the failure to the protected frame header (Golay(24,12) +
CRC-16 + 2D-pooled pilot estimation, code unchanged since before Phase 1) failing its CRC far more
often, but did not bisect which of Phase 1's five changes was responsible (explicitly flagged as
out of scope for that phase-gate task).

## Investigation

A commit-by-commit bisection (isolated `git worktree` builds, single-SNR/100-trial targeted
`coppa-bench` runs at 21 dB on Watterson-Moderate, level 1) found the floor is introduced by
**Task 5's `SyncDetector` commit** (`f085174`), not by Tasks 1-4 (carrier offset, Newman preamble,
TX conditioning, RX bandpass) as ADR-003 speculated:

| Commit | Task | Level 1 FER @ 21 dB (100 trials) |
|---|---|---|
| `453033b` | pre-Phase-1 baseline | 2% |
| `9e8e87a` | Task 1 (carrier offset) | 1% |
| `eeea46d` | Task 2 (Newman preamble) | 38% (a separate, real regression — see below) |
| `0f62930` | Task 3 (TX conditioning) | 9% (Task 3 recovers most of Task 2's hit) |
| `e441bec` | Task 4 (RX bandpass) | 3% |
| **`f085174`** | **Task 5 (`SyncDetector`)** | **31%** |
| `cc30c01` (HEAD) | — | 31% (bit-identical to `f085174`; Tasks 6-8 don't change it) |

(Task 2 alone does regress the metric sharply, via a TX/RX power-imbalance mechanism ADR-003
Task 3 already documents and fixes — that is not this ADR's subject. The regression that
*persists to HEAD* is introduced fresh at Task 5.)

Direct, same-seed comparison (forcing the detector to always pick the strongest peak, via a
`FIRST_PATH_FRACTION` ablation, then reverting) confirmed the mechanism precisely: for one
representative failing trial (`hf_standard`, level 1, Watterson-Moderate, seed `0xC1005A`, 21 dB),
first-path anchoring (offset 263 samples from a known reference point) produces **38 of 144**
header coded-bit errors — far beyond Golay(24,12)'s 3-error-per-word correction budget — while
strongest-path anchoring on the *same* transmitted signal, channel realization, and noise (offset
320 samples) produces **6 of 144** errors, decodable. Across 200 trials, offsets where the
detector's first-path logic diverges from the strongest peak average ~10 header bit errors;
offsets where it happens to coincide with (or fall back to) the strongest peak average ~0.5.

The mechanism: `hf_standard`'s 2D-pooled channel estimator (`pool_pilots` +
`LinearInterpolationEstimator`, unchanged code) linearly interpolates the complex channel estimate
between only 4 (8 after even/odd pooling) pilot subcarriers. A two-tap HF channel's composite
frequency response has a ripple/notch pattern set by the tap delay and relative amplitudes; on a
Rayleigh-faded channel, which tap is instantaneously stronger varies frame to frame, and whenever
the *later*-arriving tap happens to be the stronger one (roughly half of all Watterson draws),
anchoring the FFT window on the earlier, weaker tap leaves the interpolator to track a
composite response dominated by the *first arrival's own onset conditions and windowing/filter
interaction with the dominant, later tap* — empirically, and reproducibly, a much harder response
for a 4-pilot linear interpolator to track than the one seen when the window anchors near the
energy-dominant tap. This is specific to the *sparse*-pilot HF profiles: `hf_robust` (12 pilots)
and the VHF profiles (8 pilots, and not RX-bandpass-gated) were unaffected in the original
acceptance sweep, consistent with denser pilots tracking the same composite response adequately
regardless of anchor choice.

Two alternative hypotheses were tested and **ruled out**:
- **RC-overlap smearing the header's OFDM symbols.** `RC_OVERLAP` (24 samples) only ever writes
  into the first 24 samples of each symbol's 300-sample cyclic prefix, which `demod_ofdm_symbol`
  always strips — verified by inspection, not just assumption.
- **Cartesian (real/imaginary) pilot interpolation aliasing on a large phase ramp.** Switching
  `LinearInterpolationEstimator` to interpolate magnitude and phase (shortest angular path)
  instead of raw complex values was implemented and bench-tested; it did not move the Watterson
  FER at all (32% vs. 31%), ruling out phase-wrap aliasing as the (or a material) cause.

## Decision

`SyncDetector`'s timing refinement now **prefers the strongest correlation peak**, falling back
to the first-path (earliest-qualifying) peak only when the two are separated by more than
**half the profile's cyclic prefix** (`cp_samples / 2` — 150 samples on the 300-sample-CP HF
profiles). This:

- Restores strongest-path anchoring for every delay spread this codebase's own HF channel models
  actually produce (Watterson Good/Moderate/Poor: 24/48/96 samples), fixing the regression.
- Preserves Task 5's original safety intent for delay spreads well beyond any modeled HF
  multipath: a genuinely far, dominant late echo (more than half a cyclic prefix away) still
  anchors on the earliest arrival, so a real risk of running the FFT window past the
  ISI-free region is still guarded against.
- Costs nothing on AWGN or a single-dominant-tap channel: the two anchors coincide whenever one
  tap dominates (the common case when the "first" and "strongest" peaks are the same peak), which
  is why AWGN and the denser-pilot VHF/`hf_robust` profiles were and remain unaffected either way.

The `detector_locks_first_path_not_strongest` test (delay 96, ratio 0.6:1.0 — a scenario that,
in this codebase's own terms, is indistinguishable from a normal Watterson-Poor draw, not an
unusual edge case) is **replaced**, not just relaxed, by two tests:
- `detector_prefers_strongest_path_at_realistic_hf_delay` — same 96-sample/0.6:1.0 scenario,
  now asserting the detector locks onto the *stronger* (echo) tap, documenting the corrected
  behavior this ADR describes.
- `detector_falls_back_to_first_path_beyond_half_cp` — a 225-sample delay (`cp_samples * 3/4`,
  safely beyond the `cp/2` fallback threshold and still within one CP), asserting the original
  first-path safety property still holds there.

## Why the original decision was incomplete

Task 5's plan reasoned about sync safety in isolation (a static, noiseless two-tap bench test)
without re-running a Watterson-fading acceptance sweep against the header's own sparse-pilot
channel estimator — exactly the gap ADR-003's Consequences already flagged in general terms
("every task in this phase touched the header's channel-estimation inputs... without any task
running a full Watterson sweep"). The first-path-safety intent is real and worth keeping (hence
the `cp/2` fallback, not a blanket revert to always-strongest), but applying it unconditionally,
at delay scales that only ever occur as *normal, frequent, per-frame fading variation* in this
codebase's own HF channel model (not as a distinct, persistent, unusually-large-delay echo
geometry), traded a large, quantified, real-world-relevant loss (a hard ~30% FER floor on the
default HF speed levels under realistic fading) for protection against a scenario (multipath
delay approaching or exceeding a 300-sample/6.25 ms cyclic prefix) this codebase does not model
and amateur HF practice does not typically present at this modem's bandwidths.

## Consequences

- Sparse-pilot HF profile (`hf_standard`/`hf_wide`/`hf_narrow`) header decode under Watterson
  fading is restored for levels 1-2 (matching or exceeding pre-Phase-1 on both Moderate and
  Poor) and very close for level 3 (within normal trial-to-trial variance). Level 4 (QPSK 3/4)
  improves substantially (peak goodput roughly 1.7-2x the broken state) but retains a real,
  smaller residual gap — 72-76% of pre-Phase-1 peak goodput, not full recovery — on both
  channels. This is honestly a **partial** fix for level 4, not fully closed by this change; see
  `.superpowers/sdd/p1-fading-regression-fix-report.md` for the full before/after sweep tables
  and a discussion of why level 4 likely has a separate, smaller-scope contributing issue
  (plausibly QPSK's denser decision regions being more sensitive to residual channel-estimate
  imperfection than BPSK) not investigated further here.
- AWGN performance (which Phase 1's Task 4 RX bandpass improved) is unaffected by this change —
  re-verified by a fresh AWGN sweep alongside the fading sweeps.
- The `cp/2` fallback threshold is a judgment call, not a value derived from a specific channel
  measurement; it is chosen generously above every delay spread this codebase's Watterson presets
  produce (max 96 samples) and generously below a full cyclic prefix (300 samples), so it is not
  expected to be sensitive to precise tuning. A future channel model with meaningfully larger
  modeled delay spreads should re-examine it.
- `TIMING_BACKOFF` (30 samples, ADR-003/Task 5's own documented deviation from the plan's 60) is
  unchanged by this fix — it is a separate, already-resolved constraint (a 16-QAM clean-channel
  equalizer limit) orthogonal to which peak is chosen as the anchor.

## Related

- `docs/adr/003-phase1-waveform-break.md` — the phase this corrects a defect in.
- `docs/superpowers/plans/2026-07-03-phase1-radio-reality.md` — Task 5's original plan and
  rationale.
- `.superpowers/sdd/p1-fading-regression-fix-report.md` — full bisection data, root-cause
  verification, and before/after benchmark tables for this fix.
