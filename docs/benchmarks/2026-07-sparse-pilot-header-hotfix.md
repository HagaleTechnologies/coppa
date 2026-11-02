> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Hotfix: sparse-pilot header Watterson-fading regression (ADR-004)

Fixes the Watterson-fading regression the section above flagged but did not bisect. A
commit-by-commit bisection (isolated `git worktree` builds, single-SNR/100-trial targeted
`coppa-bench` runs at 21 dB, Watterson-Moderate, level 1) found the floor is introduced by
**Task 5's `SyncDetector` commit (`f085174`)**, not by Tasks 1–4 as this document's Phase 1
section speculated:

| Commit | Task | Level 1 FER @ 21 dB (100 trials) |
|---|---|---|
| `453033b` | pre-Phase-1 baseline | 2% |
| `9e8e87a` | Task 1 (carrier offset) | 1% |
| `eeea46d` | Task 2 (Newman preamble) | 38% (a separate power-imbalance regression, already documented above and fixed by Task 3) |
| `0f62930` | Task 3 (TX conditioning) | 9% |
| `e441bec` | Task 4 (RX bandpass) | 3% |
| **`f085174`** | **Task 5 (`SyncDetector`)** | **31%** |
| `cc30c01` (Phase 1 HEAD) | — | 31% (bit-identical to `f085174`) |

**Root cause**: `SyncDetector`'s timing refinement anchored on the *first-arriving* multipath
tap rather than the *strongest* one (a deliberate Task 5 design choice, made without a
Watterson-fading re-check). Under Rayleigh two-tap fading, roughly half the time the
later-arriving tap is instantaneously the stronger one; anchoring on the earlier, weaker tap in
that case leaves `hf_standard`'s 4-pilot 2D channel estimator (unchanged code) to track a much
harder-to-interpolate composite response than anchoring on the energy-dominant tap does. Verified
directly, not just theorized: for one representative failing trial (level 1, Watterson-Moderate,
seed `0xC1005A`, 21 dB), first-path anchoring produces 38 of 144 header coded-bit errors (Golay's
correction budget is 3 per 24-bit word); strongest-path anchoring on the *same* transmitted
signal, channel draw, and noise produces 6 of 144 — decodable. Two other hypotheses (RC-overlap
smearing the header's OFDM symbols; Cartesian pilot-interpolation phase-wrap aliasing) were
implemented and bench-tested, and both were ruled out — see
`docs/adr/004-strongest-path-timing.md` for the full investigation and
`.superpowers/sdd/p1-fading-regression-fix-report.md` for the complete trail.

**Fix**: prefer the strongest correlation peak as the timing anchor, falling back to first-path
only when the two are separated by more than half the cyclic prefix (150 samples on the
300-sample-CP HF profiles) — comfortably beyond every delay spread this codebase's own Watterson
presets produce (max 96 samples, Poor), so the original first-path safety property is preserved
for delay spreads this codebase doesn't model, while every realistic HF multipath draw now
anchors on the dominant tap. See `docs/adr/004-strongest-path-timing.md` for the full rationale,
including why Task 5's original test scenario (a 96-sample-separated two-ray channel) is, in this
codebase's own terms, indistinguishable from a normal Watterson-Poor draw rather than an edge case.

**Full 400-trial verification** (same seed/grid as `results/p1-final/{moderate,poor}.csv` and
`results/rebaseline-2026-07/{moderate,poor}.csv`; raw CSVs in `results/p1-hotfix/`):

Watterson Moderate, SNR @ FER≤10% (95% CI) and peak goodput:

| Mode (level) | Pre-Phase-1 | Post-Phase-1 (broken) | **Fixed** |
|---|---|---|---|
| BPSK 1/4 (1) | 21.0 dB / 346 bps | never clears / 249 bps | **18.0 dB / 341 bps** |
| BPSK 1/2 (2) | 21.0 dB / 688 bps | never clears / 491 bps | **18.0 dB / 686 bps** |
| QPSK 1/2 (3) | 21.0 dB / 1189 bps | never clears / 716 bps | **never clears (30 dB: FER 8.0%, hi 11.1% — just above the strict Wilson bound) / 1131 bps** |
| QPSK 3/4 (4) | never clears / 1632 bps | never clears / 633 bps | **never clears / 1234 bps** |

Watterson Poor, peak goodput (neither pre- nor post-fix ever clears FER≤10% on Poor at any
level 1–4 — this channel is deliberately harsh; peak goodput is the meaningful comparison):

| Mode (level) | Pre-Phase-1 peak | Post-Phase-1 (broken) peak | **Fixed peak** |
|---|---|---|---|
| BPSK 1/4 (1) | 265 bps | 136 bps | **233 bps (88% of pre-phase)** |
| BPSK 1/2 (2) | 418 bps | 234 bps | **404 bps (97% of pre-phase)** |
| QPSK 1/2 (3) | 811 bps | 443 bps | **738 bps (91% of pre-phase)** |
| QPSK 3/4 (4) | 772 bps | 333 bps | **555 bps (72% of pre-phase)** |

**Levels 1–2 are fully restored** (Moderate even clears FER≤10% 3 dB *earlier* than pre-Phase-1,
since the fix combines with Task 4's AWGN-noise-rejecting RX bandpass). **Level 3 is very close**
(Moderate's strict FER≤10% Wilson bound misses by ~1 point at 30 dB, 91% of pre-phase peak
goodput on Poor — within the ballpark of normal trial-to-trial variance, not the ~30%+ hard floor
this hotfix targets). **Level 4 (QPSK 3/4) improves substantially but retains a real, smaller
residual gap** (72–76% of pre-phase peak goodput, not full recovery) on both channels — a
genuinely separate, smaller-scope issue (plausibly QPSK's denser decision regions being more
sensitive to residual channel-estimate imperfection than BPSK, consistent with the
"~3 dB residual gap" already flagged for these levels earlier in this phase), not investigated
further here; flagged as a follow-up, not silently dropped.

AWGN, re-verified after the fix (`results/p1-hotfix/awgn.csv`): identical to
`results/p1-final/awgn.csv` for levels 1–4 within 1 trial of 400 at the single marginal (3 dB)
SNR point — the fix costs nothing on AWGN, exactly as expected (a single dominant path has no
first-path/strongest-path divergence to correct).

