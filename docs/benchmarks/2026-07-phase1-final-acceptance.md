> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

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

