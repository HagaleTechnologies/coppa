> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

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

