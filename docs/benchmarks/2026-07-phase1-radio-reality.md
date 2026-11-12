> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Radio-reality Phase 1: carrier offset + Newman preamble + TX conditioning

`feature/radio-reality` moves the waveform to occupy a realistic SSB passband instead of
the old full-Nyquist band, and along the way surfaced (and then repaired) a real
sensitivity regression. Documented here in full rather than only showing the final,
fixed state, because the regression's cause and the fix's limits are both worth knowing
for later dB-harvest work.

**What changed:**
1. **Carrier offset** (`carrier_offset=6`, `hf_wide` → 46 data carriers, was 50): active
   carriers now start at bin 6 (300 Hz) instead of bin 1 (50 Hz), keeping the whole
   occupied band inside a realistic ~300–2700 Hz SSB passband instead of starting at the
   very bottom of the audio band where a real radio's own highpass would destroy it.
2. **Newman-phase in-band preamble**: replaced the old full-Nyquist PN-BPSK comb with a
   Newman-phase comb confined to in-band even bins — real preamble energy now actually
   fits inside an SSB channel. The preamble is unit-RMS normalized by construction.
3. **TX conditioning** (per-section RMS leveling → RC-overlap → PAPR clip → HF-only
   601-tap bandpass → peak-normalize to 0.5 FS): added specifically to fix a regression
   the first two changes introduced (below).

**The regression (introduced by items 1–2, not caught by their own task reviews):**
those reviews checked spec compliance, symmetry, and independently re-derived the PAPR
math, but never ran a full AWGN acceptance sweep against baseline — so a real sensitivity
loss shipped undetected. Root cause: the bench's SNR convention (`awgn_ref_seeded`)
references injected noise to the *whole transmitted frame's* mean power. The Newman
preamble, being unit-RMS normalized while the header/payload body's sparse-bin IFFT
output is naturally much quieter, sits tens of dB hotter than the body with no leveling
in place — so the frame's mean power (and therefore the noise floor at any nominal SNR)
is dominated by the preamble, starving the payload of effective SNR. Measured on
`results/p1-post-task2/awgn.csv` (commit `eeea46d`, items 1–2 only, no TX conditioning
yet) against the pre-Phase-1 baseline (`results/hotfix-2026-07/awgn.csv`):

| Mode (level) | Baseline clears at | Post-Task-2 (no conditioning) clears at |
|---|---|---|
| BPSK 1/4 (1) | 12 dB | 24 dB (12 dB: 100% FER; 21 dB: 0.25%) |
| BPSK 1/2 (2) | 12 dB | 24 dB (12 dB: 100% FER; 21 dB: 0.25%) |
| QPSK 1/2 (3) | 12 dB | ~30 dB (12–21 dB: 100% FER; 24 dB: 82.25%) |
| QPSK 3/4 (4) | 12 dB | ~30 dB (9–24 dB: 100% FER; 27 dB: 52.75%) |
| 8PSK 2/3 (5) | 15 dB | **never clears in the −6…30 dB sweep** (100% FER throughout) |
| 16QAM 1/2 (6) | 15 dB | **never clears** |
| 16QAM 3/4 (7) | 18 dB | **never clears** |
| 64QAM 2/3 (9) | 27 dB | **never clears** |

A real, substantial regression — not the ~3 dB it first appeared to be from a partial
diff, but 9+ dB at the low end and complete decode failure at every tested SNR for every
mode above level 4.

**What the TX conditioning fix recovers** (`results/p1-task3-check/awgn.csv`, same seed
and trial count, all three fixes above applied):

| Mode (level) | Baseline clears at | Post-Task-2 clears at | Task-3-fixed clears at |
|---|---|---|---|
| BPSK 1/4 (1) | 12 dB | 24 dB | 15 dB |
| BPSK 1/2 (2) | 12 dB | 24 dB | 15 dB |
| QPSK 1/2 (3) | 12 dB | ~30 dB | 15 dB |
| QPSK 3/4 (4) | 12 dB | ~30 dB | 15 dB |
| 8PSK 2/3 (5) | 15 dB | never | **15 dB — matches baseline** |
| 16QAM 1/2 (6) | 15 dB | never | **15 dB — matches baseline** |
| 16QAM 3/4 (7) | 18 dB | never | **18 dB — matches baseline** (15 dB point actually *better* than baseline: 62.25% vs 94.75% FER) |
| 64QAM 2/3 (9) | 27 dB | never | **27 dB — matches baseline** |
| 64QAM 7/8 (10) | never really clears (96–99% FER at 27–30 dB even in baseline) | never | unchanged — this code rate is already at the edge of viability in every configuration |

Levels 5–10 (8PSK and above) are fully recovered to baseline parity or better. **Levels
1–4 (BPSK/QPSK, the most redundant, lowest-order modes) still clear 3 dB later than
baseline** (15 dB vs 12 dB) — a real, quantified, but much smaller residual gap than the
regression this fix repairs.

**Mechanism of the fix:** per-section RMS leveling (equalize preamble/probe/body to the
same RMS before the frame's mean power is computed) removes almost all of the
preamble-domination effect above; that leveling alone accounts for the entire recovery
shown here. The residual 3 dB at levels 1–4 is not yet root-caused further; it's left as
a known item for Phase 2's dB-harvest work (LLR calibration, real LDPC matrices) rather
than blocking Phase 1 on it, since Phase 1's purpose is radio-reality (a realistic
occupied band and preamble), not final dB optimization — and every mode still works at
every level, just at a slightly higher minimum SNR for the four most robust ones.

**A dead end worth recording so it isn't retried:** filtering a hard-clipped waveform
(the 601-tap linear-phase FIR applied after PAPR clipping) measurably regrows peaks via
ringing at the clip corners (~5 dB peak regrowth for a single clip→filter pass on
`hf_standard` level 2, confirmed by direct measurement). This looks like it should cost
average transmit power once the final stage normalizes to a fixed peak, and iterating
clip→filter a few times (the master roadmap's own anticipated "optional clip→filter
iteration") does measurably improve RMS-at-fixed-peak in isolation (~4.9 dB by 3
passes). It was implemented and benched, but produced **no net FER improvement** — small
mixed-sign differences at the noise floor of a 400-trial run (level 7 @ 15 dB: 249→245
errors, marginally better; level 9 @ 21 dB: 113→143 errors, worse; level 10: within a
few errors either way). This is expected, not a bug: `awgn_ref_seeded` references
injected noise to the actual transmitted signal's own mean power, so *any* pure
gain/peak-efficiency change is exactly self-cancelling under this bench's SNR
convention — whatever average power the conditioning chain produces, the noise floor
scales to match it at the same nominal dB. Iterative clip-filter would matter on real,
peak-limited hardware with a channel noise floor independent of transmit power; it is
invisible to (and reverted from) this codebase's own bench harness. Not implemented.

## 2026-07 — Phase 1 (radio reality): final acceptance sweep

This is Task 8's phase-gate measurement — the full 400-trial/point sweep across every speed
level for AWGN, Watterson Good/Moderate/Poor, and the new `--ssb`/`--cfo 40` bench flags,
generated after all seven prior Phase 1 tasks (carrier offset, Newman preamble, TX
conditioning, RX bandpass + SSB channel model, streaming `SyncDetector`, two-stage CFO,
`StreamingReceiver` migration) had landed. It supersedes the "still open" framing of the
~3 dB gap the "2026-07 — Radio-reality Phase 1" section above left for Phase 2 — the real
picture at the end of the phase is more mixed than that section anticipated: AWGN improved
*beyond* full recovery, but a real, previously-undetected Watterson-fading regression
appeared for the sparse-pilot HF profiles. Read on for both.

Command (seed `0x00C0FFEE` is the CLI default): `cargo run -p coppa-bench --release --
--channel {awgn|good|moderate|poor} --trials 400 [--ssb] [--cfo 40]`. Raw CSVs:
`results/p1-final/{awgn,good,moderate,poor}.csv`, `results/p1-final-ssb/awgn.csv`,
`results/p1-final-cfo40/awgn.csv`.

### (b) In-band sweeps vs pre-phase baseline — AWGN: better than full recovery

Levels 1–4 use `hf_standard` (4 pilots); levels 5/6/7/9/10 use `vhf_wide` (8 pilots, no RX
bandpass — see below). Pre-phase baseline is `results/hotfix-2026-07/awgn.csv` (same seed,
same trial count, same SNR grid, measured on `feature/hotfix-fec-correctness`):

| Mode (level) | Pre-phase SNR @ FER≤10% / ≤1% | Phase 1 final SNR @ FER≤10% / ≤1% | Delta |
|---|---|---|---|
| BPSK 1/4 (1) | 12.0 / 12.0 dB | **6.0 / 9.0 dB** | **−6 / −3 dB (better)** |
| BPSK 1/2 (2) | 12.0 / 12.0 dB | **6.0 / 9.0 dB** | **−6 / −3 dB (better)** |
| QPSK 1/2 (3) | 12.0 / 12.0 dB | **6.0 / 9.0 dB** | **−6 / −3 dB (better)** |
| QPSK 3/4 (4) | 12.0 / 12.0 dB | **9.0 / 12.0 dB** | **−3 / 0 dB (better/even)** |
| 8PSK 2/3 (5) | 15.0 / 15.0 dB | 15.0 / 15.0 dB | unchanged |
| 16QAM 1/2 (6) | 15.0 / 15.0 dB | 15.0 / 15.0 dB | unchanged |
| 16QAM 3/4 (7) | 18.0 / 18.0 dB | 18.0 / 18.0 dB | unchanged |
| 64QAM 2/3 (9) | 24.0 / 27.0 dB | 24.0 / 27.0 dB | unchanged |
| 64QAM 7/8 (10) | never clears (355 bps peak) | never clears (399 bps peak) | unchanged (both marginal) |

**Levels 1–4 (`hf_standard`) now clear 3–6 dB *earlier* than the pre-Phase-1 baseline** — not
just the "3 dB residual gap" the mid-phase Task 3 check (`results/p1-task3-check/awgn.csv`,
tasks 1–3 only, no RX bandpass yet) found and left open. The mechanism: Task 4's RX-side
bandpass (250–2850 Hz, HF-profile-gated) rejects the ~21 kHz of out-of-band noise a
full-Nyquist receiver used to integrate — a real matched-filtering-style SNR gain that
more than reverses the TX-conditioning-era regression. This is confirmed by isolating the
one variable that changed between the Task 3 check and this final sweep: levels 5–9
(`vhf_wide`, **not** RX-bandpass-gated per Task 4/Task 7's own measurements) are bit-for-bit
*unchanged* from the pre-phase baseline, while every RX-bandpass-gated HF level improved.
**Acceptance (b) is met, and exceeded, for AWGN.**

### (b) In-band sweeps vs pre-phase baseline — Watterson fading: a real, newly-found regression

Pre-phase baseline for Moderate/Poor is `results/rebaseline-2026-07/{moderate,poor}.csv`
(same seed/trials/grid). **No pre-phase "Good" baseline exists under the current
measurement conventions** (the 2026-07 re-baseline only covered AWGN/Moderate/Poor), so Good
is reported standalone below.

| Mode (level) | Moderate: pre-phase peak goodput | Phase 1 final | Poor: pre-phase peak goodput | Phase 1 final |
|---|---|---|---|---|
| BPSK 1/4 (1) | 346 bps | **249 bps** | 265 bps | **136 bps** |
| BPSK 1/2 (2) | 688 bps | **491 bps** | 418 bps | **234 bps** |
| QPSK 1/2 (3) | 1189 bps | **716 bps** | 811 bps | **443 bps** |
| QPSK 3/4 (4) | 1632 bps | **633 bps** | 772 bps | **333 bps** |
| 8PSK 2/3 (5, VHF) | 3532 bps | 3850 bps | 1271 bps | 1563 bps |
| 16QAM 1/2 (6, VHF) | 3246 bps | 4088 bps | 922 bps | 1275 bps |
| 16QAM 3/4 (7, VHF) | 1901 bps | 1593 bps | 497 bps | 480 bps |
| 64QAM 2/3 (9, VHF) | 932 bps | 424 bps | 169 bps | 51 bps |

The FER≤10% Wilson-CI threshold table tells the same story more starkly: pre-phase, levels
1–3 cleared FER≤10% on Moderate at 21 dB; **post-Phase-1 they never clear FER≤10% anywhere
in the −6…30 dB sweep**, on Moderate *or* Poor. Levels 5/6 (VHF, no RX bandpass) are flat to
slightly *better* — consistent with the AWGN finding that VHF-profile levels are unaffected
either way. **The regression is confined to the HF-profile (sparse-pilot) levels.**

**Root-caused, not just observed.** A scratch diagnostic (mirroring `header_diagnostic.rs`'s
attribution method — re-demodulating every failed frame to classify header-unparseable/CRC
vs payload-limited) on level 1 (`hf_standard`), 100 trials/cell, 30 dB:

| Channel | Frame failures | of which: no sync candidate | header CRC fail/short | payload-limited |
|---|---|---|---|---|
| Good | 7/100 | 0 | **7** | 0 |
| Moderate | 29/100 | 0 | **29** | 0 |
| Poor | 63/100 | 0 | 48 | 15 |

Sync/timing is unaffected (`SyncDetector` finds a candidate essentially every time — the new
streaming detector is not the problem). **Nearly every failure is the protected header
(Golay(24,12) + CRC-16 + 2D-pooled estimation) failing its CRC and being dropped**, at a rate
far above what the same mechanism achieved pre-Phase-1 on the same `hf_standard` profile
(`results/rebaseline-2026-07/moderate.csv` level 1 clears FER≤10% at 21 dB — that FER
includes any header loss, so header loss there was small). Re-running the same diagnostic
against `hf_robust` (12 pilots, `header_diagnostic.rs`'s own profile) at 30 dB shows header
failures back down to 0/100 (Good), 0/100 (Moderate), 0–5/100 (Poor) — matching the
originally-documented protected-header numbers almost exactly. **The regression is specific
to sparse-pilot HF profiles' header decode** (`hf_standard`/`hf_wide`/`hf_narrow`, 2–4
pilots) — the header's 2D pilot-pooled channel estimate apparently no longer resolves the
Watterson channel well enough with few pilots, post-Phase-1. None of `header_fec.rs` or
`golay.rs` (the FEC/CRC mechanism itself) changed in Phase 1 — every task from 1–7 touched
`coppa_modem.rs` (carrier layout, preamble, RX bandpass, sync, CFO), so the regression is in
how the new waveform/RX chain feeds the header's channel estimate, not in the FEC logic.
This was not caught by any single task's own review (Task 6's paired bench *did* notice
"Watterson-good never clears at level 2" and correctly ruled out Task 6 itself as the cause,
but did not trace it further) — no task before this phase-gate ran a full Watterson sweep
against the actual per-level default profiles.

**Acceptance (b) is NOT met for Watterson fading on HF-profile speed levels (1–4).** This is
a real, quantified, newly-discovered regression, not a rounding-up of a known gap — flagged
here explicitly rather than glossed over. It does not affect AWGN (which improved, above) or
the VHF-profile levels (5–9, unaffected either way). Recommended as a priority follow-up
before/alongside Phase 2, not deferred silently: likely candidates are the Newman preamble's
suitability for CFO/timing-quality inputs to the header's 2D estimate, or the narrower
`carrier_offset`d band changing the header's effective pilot geometry — not yet bisected
further (bisecting across Tasks 1–4 to isolate the exact commit was judged out of scope for
this gate task; see the coordinator's own follow-up).

**Watterson Good, standalone** (no pre-phase comparable baseline; same regression pattern
visible directly — FER floors around 6.5–7.5% for BPSK levels at 24–30 dB, dominated by the
same header-CRC mechanism per the diagnostic above, never crossing FER≤1%):

| Mode | SNR @ FER≤10% (95% CI) | SNR @ FER≤1% (95% CI) | Peak goodput (bps) |
|------|------------------------|-----------------------|--------------------|
| BPSK 1/4 | 27.0 dB | — | 327 |
| BPSK 1/2 | 21.0 dB | — | 665 |
| QPSK 1/2 | — | — | 983 |
| QPSK 3/4 | — | — | 740 |
| 8PSK 2/3 | 21.0 dB | — | 5070 |
| 16QAM 1/2 | 21.0 dB | 30.0 dB | 4555 |
| 16QAM 3/4 | — | — | 5790 |
| 64QAM 2/3 | — | — | 1372 |
| 64QAM 7/8 | — | — | 22 |

### (a) SSB-filtered + mistuned loopback at all speed levels, high SNR

The phase's own locked acceptance test (`loopback_survives_ssb_filter_and_50hz_mistune`,
`crates/coppa-protocol/src/modem/transceiver.rs`) exercises level 2 (`hf_standard`) through
`ssb_filter` + a 47 Hz mistune and passes. The `--ssb` bench sweep (400 trials, AWGN, all 9
levels, high-SNR end of a −6…30 dB grid) extends this to the full ladder:

| Mode (level) | Baseline SNR @ FER≤10%/≤1% | `--ssb` SNR @ FER≤10%/≤1% |
|---|---|---|
| BPSK 1/4 (1, HF) | 6.0 / 9.0 dB | 6.0 / 9.0 dB (unchanged) |
| BPSK 1/2 (2, HF) | 6.0 / 9.0 dB | 6.0 / 12.0 dB (FER≤1% point moves one 3 dB step; 2/400 errors at 9 dB, noise-level) |
| QPSK 1/2 (3, HF) | 6.0 / 9.0 dB | 6.0 / 9.0 dB (unchanged) |
| QPSK 3/4 (4, HF) | 9.0 / 12.0 dB | 9.0 / 12.0 dB (unchanged) |
| 8PSK 2/3 (5, VHF) | 15.0 / 15.0 dB | **never clears — 0 bps peak goodput** |
| 16QAM 1/2 (6, VHF) | 15.0 / 15.0 dB | **never clears — 0 bps peak goodput** |
| 16QAM 3/4 (7, VHF) | 18.0 / 18.0 dB | **never clears — 0 bps peak goodput** |
| 64QAM 2/3 (9, VHF) | 24.0 / 27.0 dB | **never clears — 0 bps peak goodput** |
| 64QAM 7/8 (10, VHF) | never clears | **never clears — 0 bps peak goodput** |

**HF-profile levels (1–4) meet acceptance (a) cleanly** — the small level-2 shift is 2/400
errors at the margin (Wilson-hi 0.018, essentially noise). **VHF-profile levels (5–10)
completely fail under `--ssb`, and this is expected, not a defect:** `vhf_wide` occupies bins
7–118 at 50 Hz spacing (350–5900 Hz, `data_carriers=104` + `pilot_carriers=8`), far wider than
the 300–2700 Hz SSB emulation `ssb_filter` models. VHF profiles (`phy_mode=1`) were never
designed for a narrowband SSB HF rig — they're the wider-bandwidth mode, and routing them
through an SSB-rig channel model tests a combination outside their intended scope. **Read
acceptance (a) as "met for the HF-class profiles an SSB rig actually applies to"** — which is
what the phase's own locked test (level 2, `hf_standard`) already asserted; this sweep
confirms it holds for all four HF-profile levels, not just level 2.

**`--cfo 40` sweep** (400 trials, AWGN, all 9 levels): matches the baseline exactly for every
level except one —

| Mode (level) | Baseline SNR @ FER≤10%/≤1% | `--cfo 40` SNR @ FER≤10%/≤1% |
|---|---|---|
| BPSK 1/4 (1) | 6.0 / 9.0 dB | 6.0 / 9.0 dB (unchanged) |
| BPSK 1/2 (2) | 6.0 / 9.0 dB | 6.0 / 9.0 dB (unchanged) |
| QPSK 1/2 (3) | 6.0 / 9.0 dB | 6.0 / 9.0 dB (unchanged) |
| QPSK 3/4 (4) | 9.0 / 12.0 dB | **15.0 / 18.0 dB (+6 dB)** |
| 8PSK 2/3 (5) | 15.0 / 15.0 dB | 15.0 / 15.0 dB (unchanged) |
| 16QAM 1/2 (6) | 15.0 / 15.0 dB | 15.0 / 15.0 dB (unchanged) |
| 16QAM 3/4 (7) | 18.0 / 18.0 dB | 18.0 / 18.0 dB (unchanged) |
| 64QAM 2/3 (9) | 24.0 / 27.0 dB | 24.0 / 27.0 dB (unchanged) |
| 64QAM 7/8 (10) | never clears | never clears (unchanged) |

Level 4 (QPSK 3/4) alone costs a real ~6 dB of margin at 40 Hz CFO (9 dB: 4.0% FER baseline
vs 46.5% FER at CFO=40; converges back to 0 errors by 18 dB either way) — this is a real,
mode-specific cost, not noise (Wilson CIs at 9 dB don't overlap: baseline hi=0.064 vs
cfo40 hi=0.514). Task 6's own paired bench found level 2 unaffected by CFO within 1 dB;
this sweep shows that doesn't generalize to every mode — level 4 specifically pays a
larger, still-recoverable-at-high-SNR cost. Not investigated further here (out of this
gate's scope); worth a closer look alongside the fading regression above.

### (c) Sync CPU ≤ 0.005x realtime

Already measured in Task 7's report at **0.0035x realtime** for the full `StreamingReceiver`
(10 s of noise, 512-sample chunks). Independently re-measured fresh for this gate
(`cargo run -p coppa-protocol --release --example streaming_perf`): **0.0015x realtime**
(15.3 ms wall for 10 s of input) — comfortably meeting, and in this run beating, the 0.005x
budget. **Acceptance (c) is met.**

### Acceptance summary

| Criterion | Status |
|---|---|
| (a) SSB + 47 Hz mistune decodes at all speed levels, high SNR | **Met for HF-class profiles (levels 1–4)**, the only ones an SSB channel model applies to by design; VHF-profile levels (5–10) fail under `--ssb` as expected (out-of-scope combination, not a defect) |
| (b) In-band sweeps within CI noise of pre-phase or better | **Met and exceeded for AWGN** (all levels flat or 3–6 dB better). **NOT met for Watterson fading on HF-profile levels 1–4** — a real, newly-found, root-caused header-CRC regression under fading, needing follow-up. VHF-profile levels unaffected/improved on fading too |
| (c) Sync CPU ≤ 0.005x realtime | **Met** (0.0015–0.0035x measured) |

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
`docs/adr/004-strongest-path-timing.md` for the full investigation and the complete trail.

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

