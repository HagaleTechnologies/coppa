# F — Modem fitness for purpose: performance claims, robustness, on-air readiness

Repo: `/Users/thagale/Code/coppa` @ `1c2ffc5` (main, 2026-09-05). Read-only review. Machine: Apple M4 Pro, 12 cores, **load average 11–28 throughout** (shared host) — every wall-clock number below is unreliable; FER/goodput numbers are deterministic (seeded) and unaffected by load.

## Summary

**Verdict: coppa is a well-instrumented, honestly-documented HF-modem *research codebase* with a credible PHY on AWGN, but it is not on-air ready and has never been on the air.** There is no evidence anywhere in the repo, results, wiki, or git history of a single over-the-air, two-radio, or even two-soundcard test; the repo itself says so five times ("there is still no live two-radio field test", `ARCHITECTURE.md:235` "Coppa has no measured on-air…"). Every performance claim is a single-process simulation through `coppa-channel`.

The three biggest gaps, in order:

1. **The high-speed half of the ladder is not an SSB waveform.** `CoppaCore::select_ofdm_profile` (`crates/coppa-engine/src/engine.rs:150`) routes every speed level ≥ 5 to `vhf_wide` (350–5900 Hz, 60-sample CP). Through a 300–2700 Hz SSB filter, levels 5–10 decode **zero frames at any SNR** (`results/p2-final-ssb/awgn.csv`), and even unfiltered the 60-sample CP is shorter than the CCIR-Poor 2 ms tap. So the headline "8.4 kbps at level 10" is a VHF-bandwidth number; on a real HF SSB rig the daemon's ladder tops out at **level 4, ≈1.8 kbps PHY / ≈1.4 kbps ARQ user**. Forcing `hf_standard` for the whole ladder (this review's sweep) recovers levels 5–9 (up to 3.2 kbps PHY at 18 dB) but **level 10 is undecodable through a 300–2700 Hz SSB filter even at 60 dB** (LDPC non-convergence 10/10, clean without the filter) — a never-before-characterised hard failure. The `hf_robust`/`hf_narrow`/`hf_wide` profiles in SPEC §1.1 are unreachable from the engine/daemon (the `Profile.ofdm_profile: "hf"|"vhf"` field is never read).
2. **Fading robustness is far below VARA/PACTOR class.** On Watterson CCIR-Moderate (1 ms/0.5 Hz) only BPSK levels 1–2 ever clear FER ≤ 10 % (at 12 dB); on CCIR-Poor (2 ms/1 Hz) nothing clears FER ≤ 10 % below 30 dB, level 9 never converges under any fading through 36 dB, and the simulated 10-minute ARQ session bench **drops the link on 0/5 Moderate and 0/5 Poor sessions and 2–3/5 Good sessions** — the exact failure mode the 2020 Winlink IONOS study identified as what separates ARDOP-class from VARA/PACTOR-class modems. Net session goodput in that bench is 390–530 bytes/min on Poor and 1,100–1,700 bytes/min on Good, versus VARA-2300's ≈3,500 (MPP) / ≈3,000 (MPG) bytes/min at the same ≈10 dB mid-ramp SNR in the IONOS study.
3. **Low-SNR floor is ~15–20 dB worse than the competition.** coppa's most robust level (BPSK 1/4, 44 carriers over 2.35 kHz) needs ≈ +5 dB SNR (3 kHz) on AWGN (FER 24 % at 3 dB, 99.75 % at 0 dB) and there is no narrow/low-rate mode below it. VARA HF and PACTOR-4 still move ≈130–250 bps at 0 dB in the IONOS study and PACTOR-4 specifies link maintenance to −19 dB AWGN.

Also material: the rate loop does not meet its own bar (0.914/0.762 vs > 1.0 / ≥ 0.8), the only committed "Monte Carlo" test (`tests/phase_c_loopback.rs::test_snr_fer_monte_carlo`) is vacuous (see below), main's CI is currently red on the `Security Audit` job, and the README status table is stale enough to mislead a first-time evaluator ("OFDM Partial", "QPSK not wired to engine", "EWMA predictor"). What is genuinely strong: the measurement discipline (3 kHz-referenced clean-signal SNR, ensemble-true Rayleigh Watterson, Wilson CIs, every unmet bar written down), a 1,020-line normative SPEC with 20 committed golden WAVs, NR BG2 LDPC + IR-HARQ + soft Golay header, and a very large (986 `#[test]`) unit/integration suite.

## Measured numbers

### What I ran

| Run | Command | Load avg at start / end | Result |
|---|---|---|---|
| Monte Carlo FER sweep | `cargo test --release --test phase_c_loopback test_snr_fer_monte_carlo -- --ignored --nocapture` | 11.7 / 14.2 | **FER = 0.00 (0/100) at all 71 (level, SNR) points**, 246 s wall. Vacuous — see note. |
| SSB-filtered, `hf_standard` forced, AWGN (50 trials/pt, 0–27 dB) | `coppa-bench --channel awgn --ssb --profile standard --trials 50` | 16.0 / 7.0 | L1–4 clear at 6 dB, L5/6 at 9, L7 at 15, L9 at 18; **L10: 0 decodes at every SNR** — see "Fresh SSB/forced-HF sweep" |
| Same, high-SNR check with and without SSB filter (10 trials/pt, 30–60 dB) | `--snr-min 30 --snr-max 60 [--ssb]` | ≈7 | L10 unfiltered 0/10 errors at 30/45/60 dB; L10 filtered 9/10, 10/10, 10/10, 10/10 at 30/40/50/60 dB, all `ldpc_not_converged` |
| SSB-filtered, `hf_standard` forced, Watterson Poor (50 trials/pt, 6–30 dB step 6) | `coppa-bench --channel poor --ssb --profile standard --trials 50` | ≈9 / 5 | no level clears FER ≤ 10 % (Wilson); SNR-independent LDPC floors 18–90 % from 18 dB; sync-limited at 6 dB — see Addendum |
| SSB-filtered, `hf_standard` forced, Watterson Moderate (50 trials/pt, 6–30 dB step 6) | `coppa-bench --channel moderate --ssb --profile standard --trials 50` | ≈5 | see Addendum (appended when the run finished) |
| Throughput bench | `benches/throughput.rs` (criterion: `engine_encode`, `engine_decode`, AGC, RRC, FFT, Viterbi, BPSK) | not run | Only micro-benchmarks of DSP primitives and the legacy BPSK modem; **there is no end-to-end throughput bench in `cargo bench`**. Load average made timing meaningless anyway. |

**Note on the Monte Carlo test:** it adds noise with `add_awgn` referenced to whole-frame power over the full 24 kHz Nyquist band (no 3 kHz reference), which is ≈ 9 dB optimistic versus `coppa-bench`'s convention, and it sweeps from `required_snr − 4 dB` upward. Its lowest point (−2 dB full-band ≈ +7 dB 3 kHz) is already above every level's waterfall, so 0/100 everywhere is guaranteed by construction. CLAUDE.md cites this result as evidence that levels 9/10 "converge across the whole swept SNR range" — true but uninformative. It also still labels level 10 "rate=7/8" (stale `SPEED_LEVELS` metadata, SPEC §12).

### Committed results (`results/p2-final*/`, 400 trials/pt, 3 kHz-referenced clean-signal SNR, −6…30 dB step 3, default per-level profile routing → levels ≥ 5 on `vhf_wide`)

Derived from the CSVs (`fer ≤ 0.10` first crossing; peak goodput = payload bits × (1−FER) / frame airtime):

| Level | Mode | AWGN FER≤10 % | AWGN + SSB filter | AWGN + 40 Hz CFO | Watt. Good | Watt. Moderate | Watt. Poor | Peak goodput (bps) |
|---|---|---|---|---|---|---|---|---|
| 1 | BPSK 1/4 | 6 dB (24 % FER @ 3) | 6 | 6 | 12 | 12 | 30 | 352 |
| 2 | BPSK 1/2 | 6 | 6 | 6 | 12 | 12 | never | 709 |
| 3 | QPSK 1/2 | 6 | 6 | 9 | never | never | never | 1229 |
| 4 | QPSK 3/4 | 6 | 6 | **never** (994 bps peak) | never | never | never | 1849 |
| 5 | 8PSK 2/3 (vhf_wide) | 15 | **never / 0 bps** | 15 | 21 | never | never | 5082 |
| 6 | 16QAM 1/2 (vhf_wide) | 15 | **never / 0 bps** | 15 | 21 | never | never | 4555 |
| 7 | 16QAM 3/4 (vhf_wide) | 15 | **never / 0 bps** | 15 | never | never | never | 6852 |
| 9 | 64QAM 2/3 (vhf_wide) | 18 (21 post-HARQ-fix) | **never / 0 bps** | 18 | never | never | never | 6709 |
| 10 | 64QAM 5/6 (vhf_wide) | 24 | **never / 0 bps** | 24 | never | never | never | 8450 |

Per the milstd table (BENCHMARKS.md "Phase 3 Task 8", `hf_standard` forced for every level): under Watterson **Good**, levels 3+ never clear FER ≤ 10 % in a 6–36 dB sweep; level 2 clears at 12–18 dB.

### Simulated ARQ session bench (`session.rs`, level 2 fixed, window 8, 150 ms turnaround, 20→0→20 dB ramp, 10 min)

| Preset | Drop-free sessions | Net goodput (bytes/min) | ≈ bps |
|---|---|---|---|
| Good | 2/5 | 1,314 (COP-3 re-run: 1,103–1,703) | 147–227 |
| Moderate | 0/5 | 1,039 (992–1,491) | 132–199 |
| Poor | 0/5 | 342 (383–538) | 51–72 |

### Derived effective user throughput per level (analytic, `hf_standard`, from `frame_airtime_s` arithmetic)

Frame = 3 preamble/probe + 4 header + ⌈⌈1944/bps⌉/44⌉·cw payload symbols × 26.25 ms; payload = (k_used/8 − 4)·8·cw bits. ARQ cycle = 8 data frames + 2 turnarounds + 1 ACK frame (ACKs are full frames encoded at the *current* speed level via `engine.encode_bytes`; modelled here at L2 = 1.365 s, the worst realistic case). "Daemon" column uses the shipped defaults: `ptt_pre 50 ms + ptt_tail 200 ms + turnaround 150 ms` per switch and `max_frames_per_turn = 4`.

| Level | Frame airtime (1 cw / 8 cw) | PHY goodput 1 cw / 8 cw (bps) | Spectral eff. (b/s/Hz in 2.35 kHz) | ARQ user, win 8, 150 ms (bps) | ARQ user, daemon defaults, 4 frames/turn (bps) |
|---|---|---|---|---|---|
| 1 | 1.365 s / 9.63 s | 328 / 372 | 0.14 | 285 / 364 | 215 / 346 |
| 2 | 1.365 / 9.63 | 686 / 777 | 0.29 | 595 / 761 | 450 / 723 |
| 3 | 0.787 / 5.01 | 1189 / 1493 | 0.51 | 940 / 1434 | 622 / 1307 |
| 4 | 0.787 / 5.01 | 1808 / 2272 | 0.77 | 1430 / 2182 | 947 / 1988 |
| 5* | 0.578 / 3.33 | 2189 / 3033 | 0.93 | 1609 / 2855 | 977 / 2497 |
| 6* | 0.499 / 2.70 | 1877 / 2769 | 0.80 | 1324 / 2572 | 770 / 2189 |
| 7* | 0.499 / 2.70 | 2855 / 4213 | 1.21 | 2015 / 3912 | 1172 / 3331 |
| 9* | 0.394 / 1.86 | 3210 / 5426 | 1.37 | 2100 / 4881 | 1139 / 3919 |
| 10* | 0.394 / 1.86 | 4023 / 6799 | 1.71 | 2632 / 6116 | 1427 / 4912 |

\* Not reachable on `hf_standard` from the engine today (routed to `vhf_wide`). The `vhf_wide` numbers the CSVs report (L10: 8.3 kbps 1 cw / 16.1 kbps 8 cw PHY) need 5.55 kHz of audio bandwidth.

Fixed overhead per single-codeword frame is 7 of 52 symbols at BPSK (13 %) but 7 of 15 at 64-QAM (47 %) — multi-codeword frames (up to 8) are implemented but the daemon's ARQ path still sends one codeword per frame (nothing in `event_loop.rs` sets `codewords > 1`; per ADR-007 ACK addressing is whole-frame only).

### Side-by-side with published competitor numbers (SNR in 3 kHz)

Competitor figures read from the Winlink IONOS "Take Three" study charts (bytes/min → bps ÷ 7.5), CCIR Poor = "MPP" (1 Hz / 2 ms), Good = "MPG" (0.1 Hz / 0.5 ms) — same presets as `coppa-channel`.

| Condition | coppa (SSB-realistic: L ≤ 4 `hf_standard`, ARQ user est.) | coppa (if L5–10 were on `hf_standard`, 8 cw) | VARA HF 2300 | PACTOR-4 (2.4 kHz) | ARDOP 2000 |
|---|---|---|---|---|---|
| AWGN 30 dB | ≈1.4 kbps (L4) | ≈6.1 kbps (L10, AWGN only) | ≈6.1 kbps (46,000 B/min) | ≈5.3 kbps (40,000) | ≈1.3 kbps (10,000) |
| AWGN 20 dB | ≈1.4 kbps (L4) | ≈3.9 kbps (L7) | ≈3.1 kbps (23,500) | ≈5.3 kbps (40,000) | ≈0.7 kbps (5,500) |
| AWGN 10 dB | ≈1.4 kbps (L4, clears at 6 dB) | same | ≈0.67 kbps (5,000) | ≈2.1 kbps (15,500) | ≈0.2 kbps |
| AWGN 5 dB | ≈0.2 kbps (L1 @ 24 % FER) | — | ≈0.27 kbps (2,000) | ≈0.53 kbps (4,000) | ≈0.07 kbps |
| AWGN 0 dB | **0** (L1 FER 99.75 %) | — | ≈0.13 kbps (1,000) | ≈0.2 kbps (1,500) | ~0 |
| Lowest usable SNR | **≈ +4 dB** | — | < 0 dB (IONOS); FSK levels 18–175 bps | **−19 dB** (SCS spec) | ≈ 0–5 dB |
| CCIR Poor 20 dB | L4 989 bps PHY @ ≈45 % FER → ARQ session ≈0.05–0.07 kbps; **links drop** | L3–4 never clear FER 10 % | ≈1.5 kbps (11,000), no drops | ≈2.9 kbps (21,500), no drops | ≈0.13 kbps, drops |
| CCIR Poor 10 dB | ≈0 (L1 FER > 60 %) | — | ≈0.47 kbps (3,500) | ≈1.3 kbps (10,000) | ~0 |
| CCIR Good 20 dB | L2 (12 dB clear) ≈0.6 kbps; ARQ session ≈0.15–0.23 kbps | L3+ never clear FER 10 % (milstd) | ≈1.6 kbps (12,000) | ≈2.9 kbps (21,500) | ≈0.2 kbps |
| Link survival on multipath | **0/5 Moderate, 0/5 Poor sessions survive** | — | "never lost a connection" | "never lost a connection" | "often required multiple runs" |

Sources: Winlink IONOS comparison (Nov 2 2020) — https://winlink.org/sites/default/files/downloads/a_winlink_digital_mode_performance_comparison_based_on_the_ionis_sim_hf_vhf_channel_simulator_-_november_2_2020_0.pdf ; VARA HF level table (17 levels, 18 bps FSK → 8,489 bps 32-QAM/2750 Hz; ≈7 kbps in 2300 Hz) — https://www.sigidwiki.com/wiki/VARA_HF ; PACTOR-4 5,512 bps in 2.4 kHz, 10 levels, link maintained to −19 dB AWGN — https://www.scs-pactor.com/en/pactor-4 and https://cms.scs-ptc.com/pactor-4/ ; ARDOP spec (max 2000 Hz, 4/8PSK, 4FSK) — https://winlink.org/sites/default/files/downloads/_ardop_specification.pdf . MIL-STD-188-110 Table XVI anchors as quoted in `docs/analysis/2026-07-03-world-class-gap-analysis.md` (2400 bps @ 18 dB Poor; 75 bps @ 2 dB).

**Reading:** on AWGN between ≈6 and ≈15 dB coppa's L4 is actually competitive with VARA (and, with L5–10 restored to an SSB profile plus 8-codeword frames, its 20–30 dB AWGN peak would match VARA's). The problems are the two ends and the middle that matters: nothing below +4 dB, nothing survives CCIR-Poor, and the link drops under fading. Spectral efficiency at the top (1.7 b/s/Hz PHY at L10 `hf_standard`) is fine; the missing ingredient is fading diversity, not constellation size.

### Fresh SSB/forced-HF sweep (this review, 50 trials/pt, 3 kHz-referenced, seed 12648430, load 7–16)

This is the table `results/` does not contain: every level on the SSB-passband `hf_standard` profile, through the 300–2700 Hz `ssb_filter`, i.e. what a real rig would see.

**AWGN + SSB filter, `hf_standard` at every level** (`--channel awgn --ssb --profile standard --trials 50`, 0–27 dB step 3):

| Level | Mode | FER≤10 % | Peak goodput (bps) | Failure mode at high SNR |
|---|---|---|---|---|
| 1 | BPSK 1/4 | 6 dB | 328 | — |
| 2 | BPSK 1/2 | 6 | 686 | — |
| 3 | QPSK 1/2 | 6 | 1189 | — |
| 4 | QPSK 3/4 | 6 | 1808 | — |
| 5 | 8PSK 2/3 | 9 | 2189 | — |
| 6 | 16QAM 1/2 | 9 | 1877 | — |
| 7 | 16QAM 3/4 | 15 | 2855 | — |
| 9 | 64QAM 2/3 | 18 (34 % @ 15) | 3210 | LDPC non-convergence below 18 |
| 10 | 64QAM 5/6 | **never** | **0** | **50/50 `ldpc_not_converged` at 15, 18, 21, 24, 27 dB** |

Follow-up (10 trials/pt): level 10 on `hf_standard` **without** the SSB filter decodes 0/10 errors at 30, 45 and 60 dB (peak 4023 bps); **with** the filter it fails 9/10 at 30 dB and 10/10 at 40, 50 and 60 dB, always `ldpc_not_converged`, never sync/header. So level 10 is deterministically undecodable through a 300–2700 Hz SSB passband on the HF profile — a hard failure, not a late waterfall. Likely mechanism (not verified): `hf_standard`'s top carriers sit at 2650–2700 Hz, inside the filter's transition band; 64-QAM rate-5/6 has no margin for a few attenuated edge carriers. Whatever the cause, the shipped ladder's top HF-capable level on a real rig is **level 9 at 18 dB / 3.2 kbps PHY (1 codeword)**, and levels 5–9 only if item 2 (profile routing) is fixed. Note the per-level goodputs match the analytic `hf_standard` table above exactly.

**Watterson Poor / Moderate + SSB filter, `hf_standard` at every level** (`--trials 50`, 6–30 dB step 6): see addendum at the end of this file.

## What's already good / genuinely strong

- **Measurement honesty is best-in-class for an amateur modem project.** 3 kHz-referenced SNR against *clean* signal power (`awgn_ref_seeded`), ensemble-true Rayleigh Watterson with ITU-R F.1487 two-sigma Doppler convention, Wilson 95 % CIs required to clear a threshold, and every failed acceptance bar recorded (ADR-006/008, COP-2/4/5, CLAUDE.md). Three separate bench-harness bugs that flattered results (per-frame power renormalisation, full-band SNR, stale IR-HARQ accumulator) were found and fixed *by the project itself*.
- **Channel model is the right one.** `coppa-channel::watterson` is a genuine two-tap ITU-R F.1487 tapped-delay-line with Gaussian-Doppler Rayleigh taps, presets exactly CCIR Good/Moderate/Poor (0.5 ms/0.1 Hz, 1 ms/0.5 Hz, 2 ms/1 Hz), plus a 601-tap 300–2700 Hz SSB filter and clean wideband CFO. Per-tap gains are exposed for ground-truth oracles.
- **Modern FEC/estimation stack.** NR BG2 LDPC (cross-validated against Sionna and srsRAN), circular-buffer rate matching with real IR-HARQ RV cycling and LLR combining, known-pad pinning, exact max-log LLR scaling, soft-ML Golay(24,12)+CRC-list header, delay-domain estimator + Kalman tracker, one-round turbo re-estimation, SCO tracking measured to ±120 ppm with no new failure mode (COP-3).
- **Spec + golden vectors.** `docs/SPEC.md` is normative, cites every constant to a source line, has a 4-bit wire version in the header and version-keyed preamble/PN; 20 golden WAVs (levels {1,2,5,6,9} × {clean, AWGN 12, Poor 25, SSB+CFO}) are CI-checked. An independent reviewer already caught one wire-breaking spec error (interleave direction). A second implementation is plausible from SPEC.md alone.
- **Field-readiness plumbing exists**: real DTR/RTS/GPIO/rigctld PTT that hard-errors on bad config, two-tone TUNE with an ALC procedure in `docs/OPERATING.md`, busy-channel courtesy gate, 9-minute station-ID timer, beacon mode, VARA-style SNR/PTT/BUFFER/BUSY telemetry, WebSocket waterfall, `max_tx_duration_s` safety unkey.
- **Test discipline**: 986 `#[test]`s, proptest round-trips (2 regression seeds saved, both trivial `[254]`), loss-injection daemon↔daemon tests for every droppable CP-negotiation frame, regression tests that were verified to fail pre-fix, full-suite CI ≈14.5 min.
- CFO tolerance (±50 Hz = one subcarrier spacing) is verified under fading and the L4×CFO regression was root-caused and fixed.

## Hit list

Impact H/M/L, Effort S/M/L. Ranked.

| # | Item | Category | Impact | Effort | Evidence |
|---|---|---|---|---|---|
| 1 | **Do a two-computer audio-cable loopback test, then a real on-air test, and publish the log.** Zero OTA/two-soundcard evidence exists; the repo admits it 5×. Until this exists every number is a simulation claim. | evidence-gap | H | M | `grep -rn "no live two-radio"` → 5 hits; `ARCHITECTURE.md:235`; no `*.wav` from a radio anywhere in `results/` |
| 2 | **Stop routing levels ≥ 5 to `vhf_wide` for HF.** Give the ladder an HF profile at every level (or make routing follow `Profile.ofdm_profile`, which is currently a dead field). Levels 5–10 decode 0 frames through an SSB filter; forced onto `hf_standard` levels 5–9 work at 9/9/15/18 dB (this review's sweep). | robustness | H | S | `engine.rs:150-159`; `results/p2-final-ssb/awgn.csv` L5–10 all-fail; `profiles.rs` `ofdm_profile` never read; milstd/session had to force `hf_standard`; fresh sweep above |
| 2b | **Level 10 (64-QAM 5/6) is undecodable through a 300–2700 Hz SSB passband on `hf_standard` at any SNR up to 60 dB** (LDPC non-convergence 10/10), while decoding cleanly unfiltered. Either drop level 10 from the HF ladder, narrow `hf_standard`'s top edge (2700 → ≤ 2600 Hz), or add edge-carrier erasure handling. Never characterised before because the SSB sweep only ever ran with default (VHF) routing. | robustness | H | S | `results/review-2026-09-05/awgn-ssb-std{,-hi}/awgn.csv` (this review); `results/p2-final-ssb` never exercised `hf_standard` at L10 |
| 3 | **Fade-diversity / time interleaving across ≥ 1 coherence time** (the one untried lever CLAUDE.md names). L9 perfect-CSI bound admits 91.6 % of Good frames at 30 dB vs 15 % real; every level ≥ 3 fails FER 10 % on Good/Moderate; no within-frame interleaver can save a 1.365 s BPSK frame from a 1 Hz fade. | perf | H | L | COP-4 section; ADR-006 "Fade-diversity interleaving is now the one genuinely untried lever"; `results/p2-final/moderate.csv` |
| 4 | **Add a genuine weak-signal mode** (narrow, ≤ 100 bps, long coherent integration, e.g. the existing but unreachable `hf_narrow` 8-carrier profile with BPSK 1/4 or a 4-FSK/chirp level). Floor today is ≈ +4 dB (3 kHz); VARA/PACTOR work below 0 dB, PACTOR-4 to −19 dB. | perf | H | M | `results/p2-final/awgn.csv` L1: FER 0.9975 @ 0 dB, 0.24 @ 3 dB; IONOS charts; SCS spec |
| 5 | **Fix link-drop under fading: ARQ give-up policy + rate loop must not exhaust 5 retries in a fade trough.** 0/5 Moderate & Poor sessions survive; Good 2/5. Consider adaptive max-retransmit, drop-to-L1-before-giving-up, and IONOS-style "never drop while a header decodes". | robustness | H | M | BENCHMARKS.md `session` tables; `DEFAULT_MAX_RETRANSMIT = 5`, `MIN_RTO 1 s / MAX 60 s` in `arq.rs:75-99` |
| 6 | **Wire multi-codeword frames into the daemon's ARQ path** (currently 1 codeword/frame; 47 % fixed overhead at 64-QAM). Needs per-codeword ACK addressing or at least block sizing. | perf | H | M | ADR-007 decisions 4–5; no `codewords:` > 1 in `event_loop.rs`; analytic table above (L10 4.0→6.8 kbps PHY) |
| 7 | **Build a 2-host, 2-soundcard bench harness** (`coppa bench-otacable`) that runs the real daemon↔daemon session over physical audio, records WAVs, and scores bytes/min + drops; run it in CI-adjacent nightly on real hardware. | evidence-gap | H | M | Only in-process `EventLoop` pair tests exist (`event_loop.rs`); `daemon_loopback.rs` is 74 lines of engine round-trip |
| 8 | **Add a daemon↔daemon session test through Watterson + SSB filter + CFO + SCO** (real `decode_and_dispatch_audio` path). Today the only fading-exercising integration test is `tests/short_cp_profile.rs`; `session.rs` bypasses the daemon. | test-gap | H | M | `grep -rl watterson crates/coppa-daemon tests` → only `short_cp_profile.rs`; Phase 4 found the UTF-8-drop bug precisely because nothing exercised this path |
| 9 | **Replace the vacuous Monte Carlo test** with a 3 kHz-convention sweep that spans each level's waterfall and asserts a threshold with a Wilson CI; run it (reduced trials) in CI. | test-gap | H | S | `phase_c_loopback.rs:191-224,336-388`: full-band `add_awgn`, sweep starts at `required−4`; measured 0/100 at all 71 points |
| 10 | **Rate loop still misses its bar (0.914 / 0.762 vs > 1.0 / ≥ 0.8)** and oscillation/dwell behaviour on fast fading is the diagnosed cause; active probing is built but not wired into the daemon. Wire probing, and add an oscillation-rate guard. | perf | M | M | CLAUDE.md RateLoop bullet; `rate_loop.rs` `with_probing` has no daemon caller; `closed_loop_arq.rs` |
| 11 | **Publish an honest headline table in README/BENCHMARKS "Current performance"** — SSB-filtered, `hf_standard` at every level, AWGN + Good/Moderate/Poor, ARQ user bps and drop rate, with the IONOS side-by-side. Today README claims nothing measurable and BENCHMARKS "Current performance" is buried under 2,000 lines of diary. | evidence-gap | M | S | README.md status table; BENCHMARKS.md line 1999 |
| 12 | **README status table is stale/wrong** ("OFDM Partial… sync has CFO limitations", "QPSK/8PSK/QAM not wired to engine", "Channel prediction EWMA", "LDPC 6 rates", "9 speed levels" in CLAUDE.md). A sysop evaluating the project will bounce. | spec | M | S | README.md:10-40 vs SPEC §6; CLAUDE.md:59 |
| 13 | **CI on `main` is red** — `Security Audit` (rustsec/audit-check) fails with "Resource not accessible by integration" (needs `checks: write`), so the `CI` summary gate is failure on the last two main runs. | test-gap | M | S | `gh run view 33764188558`: Security Audit failure 14:02:08Z; summary `CI: failure` |
| 14 | **No impulsive-noise, clipping/ALC, AGC, frequency-tilt, or group-delay-ripple channel model.** Real SSB rigs and lightning QRN are absent from every bench; the PAPR clip is TX-side only. | robustness | M | M | `coppa-channel/src/lib.rs` exports only AWGN, `frequency_shift`, `freq_offset`, `ssb_filter`, sinusoidal `fading`, `watterson` (grep impulse/clip/agc = 0) |
| 15 | **CFO envelope ±50 Hz is one subcarrier spacing; typical mistune + drift on 20 m can exceed it** (VARA tolerates more; ARDOP ±100 Hz "wide" search). Add an integer-bin ambiguity search over the preamble comb or an ALE-style wider acquisition. | robustness | M | M | SPEC §13 "MUST tolerate ±50 Hz"; ADR-003 decision 6; `integration_test.rs:120` still `#[ignore = "OFDM sync has no CFO correction yet"]` (stale) |
| 16 | **Turnaround/latency is unmeasured.** Audio poll is 20 ms, retransmit poll 500 ms, PTT pre 50 ms + tail 200 ms, ACK is a full ≥ 0.39–1.365 s frame at the *data* level; no bench reports RX-last-sample→PTT-on. Add a measured turnaround figure and a short dedicated ACK frame. | perf | M | M | `event_loop.rs:647-654`, `1330-1345`; `coppad.toml.example`; `arq.rs:82` `DEFAULT_TURNAROUND 150 ms` is the *model*, not a measurement |
| 17 | **`turnaround_ms: 500` and `Profile.arq_window/max_payload/ofdm_profile` are dead config** — the daemon uses `ArqConfig::default()` (window 8, RTO 5 s, 150 ms) regardless. | robustness | M | S | `event_loop.rs:174,357` (field never read); `event_loop.rs:234,1386` `ArqConfig::default()`; `profiles.rs` |
| 18 | **Block-ACK cadence never implemented** — one full-frame ACK per decoded data frame. At L10 an ACK costs as much air as the data frame. | perf | M | S | ADR-008 decision 4 deviation; `event_loop.rs:1322` comment |
| 19 | **Level 4 (QPSK 3/4) residual fading gap (72–76 % of pre-Phase-1 goodput) and levels 3–6 Moderate regression (L6 @ 30 dB: 10 % → 40 % FER) shipped unresolved.** | perf | M | M | `wiki/pages/watterson-level-4-gap.md`; ADR-006 Consequences |
| 20 | **Sync detector runs on unfiltered samples (−9 dB detection margin on noisy channels, by design for CPU).** Recover with a cheap decimated pre-filter. | perf | M | S | ADR-003 Consequences "costs ~9 dB of detection margin" |
| 21 | **`CpGate` threshold (2.5 ms) is mis-set vs the estimator's 0.417 ms delay grid (Poor reads as "calm"); `BusyGate` +6 dB / 60 % bins never tuned on recordings.** Busy-detect false positives/negatives on a real band are unknown. | robustness | M | S | CLAUDE.md COP-2 paragraph; `busy_gate.rs:27-29` "no sweep against real recordings" |
| 22 | **No sub-band / narrow profile reachable; 500 Hz mode absent.** VARA-500 & PACTOR-2 dominate below 14 dB in IONOS; coppa cannot occupy a 500 Hz slot at all. | perf | M | M | SPEC §1.1 `hf_narrow` 350–800 Hz exists; unreachable (item 2) |
| 23 | **Golden vectors cover only levels {1,2,5,6,9}; the Poor vector is seed-selected (9–14 % success rate).** Add levels 3,4,7,10, multi-codeword, RV 1–3 retransmission vectors, a CONNECT/ACK PDU vector, and a real-radio-captured WAV once item 1 exists. | spec | M | S | SPEC §14; BENCHMARKS golden-vector UPDATE |
| 24 | **SPEC covers PHY/FEC only; MAC/transport/ARQ/session wire format is not normative** (ack bitmap widened u8→u32 was a wire break documented only in an ADR; CP-negotiation kinds 0x01–0x03, `MacPdu`, `TransportPdu`, session handshake not in SPEC). A second implementation could decode frames but not hold a session. | spec | M | M | SPEC §11 "MAC layer is not part of this spec"; ADR-008 decision 4 |
| 25 | **No protocol version negotiation / capability exchange** — profile & version must match out-of-band (SPEC §1.3); no in-band CP-capability negotiation ("disjoint subsystems" scope cut). | spec | M | M | SPEC §0.1, §1.3; CLAUDE.md CP-negotiation paragraph |
| 26 | **Sample-clock offset is only modelled as ideal linear resampling.** Real USB codecs have jitter/drift wander and buffer glitches; COP-3 is fine as far as it goes (±120 ppm) but ≥10 s multi-codeword frames at 100 ppm drift 48 samples — verify on hardware (item 1). | robustness | L | S | BENCHMARKS COP-3 ("not a claim of live two-radio or hardware-clock validation"); math: 100 ppm×10 s×48 kHz = 48 samples |
| 27 | **`vhf_wide` CP (60 samples = 1.25 ms) is shorter than CCIR-Poor's 2 ms tap** — even for genuine VHF use through a fading path this profile is fragile; document it as line-of-sight-only or lengthen. | robustness | L | S | SPEC §1.1; COP-4 "96-sample delayed tap exceeds the 60-sample CP" |
| 28 | **`SWITCH_PROBATION_SECS = 180` and CP negotiation defaults are derived, never swept**; the three-flag enable conjunction is a footgun for operators. | robustness | L | S | CLAUDE.md; ADR-008 follow-on 2 |
| 29 | **Preamble PAPR is 4.9–5.8 dB (not the ~3 dB design claim) and PAPR targets up to 14 dB at 64-QAM** — on a real rig with ALC this will be clipped; no ALC/compression model verifies the clip schedule. | robustness | L | M | SPEC §3.1 note; §6 PAPR table |
| 30 | **Header + probe give no per-frame SNR for the *peer* except via ACK-carried level**; there is no in-band channel-quality report for the remote side's RX, so a one-way-bad path cannot be diagnosed by the sysop. | robustness | L | M | `TransportPdu::new_ack_with_rate` carries 1 byte level only |
| 31 | **Stale `#[ignore]`d CFO test and stale `CodeRate::Rate7_8` label for L10** mislead readers about capability. | test-gap | L | S | `tests/integration_test.rs:120`; SPEC §12; MC log "rate=7/8" |
| 32 | **`cargo bench` has no end-to-end OFDM throughput/latency benchmark** — only legacy BPSK/Viterbi/FFT micro-benches; the "throughput bench" the brief refers to does not exist. | test-gap | L | S | `benches/throughput.rs` |
| 33 | **Bench examples are never run in CI** (no regression protection on any FER/goodput figure; COP-2 explicitly notes it). Add a nightly reduced-trial bench job with a drift alarm. | test-gap | M | M | CLAUDE.md COP-2 caveat "CI never runs bench examples" |
| 34 | **Station ID is only prepended to a real transmission** — an idle connected session with no traffic for >10 min never IDs (Part 97.119 requires ID at end of communication and every 10 min *while transmitting*, so this is arguably fine, but a disconnect frame should carry ID; verify). | safety | L | S | `coppad.toml.example` `[station_id]` comment |
| 35 | **Unauthenticated control plane** (documented) plus `max_tx_duration_s = 30` is the only TX safety; no RF-timeout on busy-gate defer wait, no PTT watchdog if the daemon panics with PTT asserted. | safety | M | S | `coppad.toml.example` WARNING; `event_loop.rs:1554-1582` |
| 36 | **`session` bench acceptance was lowered to "≥ 2/5 Good drop-free" (COP-5)** — a regression floor, not a target. Keep the original zero-drop goal visible as the product target. | evidence-gap | L | S | ADR-008 COP-5 update |
| 37 | **`hf_robust` (12 pilots) is never reachable yet the delay-domain estimator's biggest weakness is sparse pilots**; either wire it or delete it from SPEC's SHOULD list. | spec | L | S | SPEC §1.1; ADR-004 mechanism |
| 38 | **Watterson module doc still claims "coherence time ~1–10 s, comparable to or longer than a frame"** — CLAUDE.md admits a L2 frame decorrelates to ~10 % by frame end on Moderate. Fix the doc; it misleads future estimator work. | spec | L | S | `watterson.rs:10-13` vs CLAUDE.md fading root-cause paragraph |
| 39 | **Decode CPU for LDPC is 1.5–4.3× the old codec; 4 levels exceed the 3× budget** — fine on M4, relevant for the Pi-class targets the gap analysis names. | perf | L | M | ADR-005 table; BENCHMARKS COP-6 |
| 40 | **No interop test against a second decoder implementation** (e.g. a Python/NumPy reference decoding the golden WAVs) — the "could someone build a second implementation" claim is untested. | spec | M | M | SPEC §14 says SHOULD; nothing in `tools/` |

## Proposed on-air validation protocol

Pass criteria use the repo's own conventions (Wilson 95 % upper bound must clear the target). Every stage records raw 48 kHz WAVs on both ends and commits a manifest under `results/ota/<date>/`.

1. **Two-host audio-cable loopback (the standard bench test).** Two computers, two USB soundcards, TRS cables TX→RX both ways, no radio. Run `coppad` on each with `arq_enabled = true`, `ptt_method = "none"`. (a) Decode 20 golden WAVs played through the cable: **20/20**. (b) 10-minute ARQ session, fixed L2 then RateLoop: **0 drops, goodput ≥ 90 % of the simulated AWGN-30 dB figure (≈ 550 bps user at L2)**. (c) Repeat with a −50 ppm / +50 ppm software resample on one side (or two cards of different make): **0 drops**. (d) Measure RX-last-sample → PTT-assert with a GPIO/scope: report the number; target **≤ 250 ms**. This stage also settles whether item 2's `vhf_wide` frames pass a real codec's anti-alias filter.
2. **Cable loopback through a hardware SSB channel simulator or `hf_channel_sim`-style software path** (e.g. the Winlink IONOS Teensy simulator, or a `coppa-channel` process piped between the two soundcards) at WGN 30/20/10/5 dB and MPG/MPP 30/20/10 dB. Score bytes/min and drops exactly as the IONOS paper does so the numbers are directly comparable. **Pass: 0 drops at any SNR where a CONNECT succeeds on WGN and MPG; publish MPP as measured.**
3. **Local VHF/FM with two rigs** (2 m simplex, 1–5 W, same room or across town, FM so the channel is flat): validates PTT sequencing, TUNE/ALC, audio-level misalignment (run at −20 dB / −10 dB / 0 dB / +6 dB drive relative to ALC onset), busy-gate behaviour with a third station keying up, station-ID timer. **Pass: 30-minute session, 0 drops, ID observed ≤ every 9 min, no splatter on an adjacent-channel receiver at rated drive.**
4. **HF NVIS, 40/80 m, 50–300 km, daytime** (mild multipath, moderate SNR). One station is a Winlink-style gateway using the VARA-TCP API from a real client (Pat or RMS Express-style flow). Record SNR telemetry, level trajectory, retransmits. **Pass: complete a 10 kB message both directions with 0 drops; report goodput vs the reported SNR against the simulated Good curve.**
5. **HF one-way via WebSDR/KiwiSDR** (e.g. transmit CQ/beacon frames at fixed levels 1–4 into a distant KiwiSDR, capture the IQ/audio, decode offline with `coppa rx`). This gives a cheap, repeatable long-path multipath dataset: **record FER per level vs KiwiSDR-reported SNR over ≥ 200 frames per level; commit the WAVs as the first real-channel golden set.**
6. **HF DX / poor-conditions campaign** (20 m greyline, 1–3 kkm, and a deliberately QRN-heavy summer 80 m evening). Same scoring as stage 4. **Pass criterion is "publish", not "win": tabulate drops, bytes/min, lowest SNR at which CONNECT succeeded, and put it next to the IONOS VARA/PACTOR curves in BENCHMARKS.md.**
7. **Regression hook.** Fold stage-1 (cable) into a nightly job on a dedicated two-soundcard host so the on-air-relevant numbers get the same drift protection the simulated ones lack (item 33).

Until stage 1 is done, the honest one-line status for a sysop is: *"Simulated only; SSB-realistic ladder tops out at ≈1.4 kbps user; does not yet hold a link through CCIR-Moderate fading."*

## Addendum — fresh Watterson sweeps through SSB filter on `hf_standard` (this review, 50 trials/pt, 6–30 dB step 6)

**CCIR Poor (2 ms / 1 Hz) + SSB filter, `hf_standard` every level** (`results/review-2026-09-05/poor-ssb-std/poor.csv`). FER per SNR; failure mode in brackets (sync / LDPC):

| Level | 6 dB | 12 dB | 18 dB | 24 dB | 30 dB | Peak goodput | Floor character |
|---|---|---|---|---|---|---|---|
| 1 | 0.36 (16/2) | 0.14 (6/1) | 0.06 (3/0) | 0.18 (1/8) | 0.08 (2/2) | 309 bps | the only level near FER 10 %; sync-limited below 12 dB |
| 2 | 0.56 | 0.40 | 0.30 | 0.32 | 0.18 | 562 | flat ≈ 20–30 % LDPC floor from 18 dB |
| 3 | 0.60 | 0.46 | 0.28 | 0.26 | 0.24 | 903 | flat ≈ 25 % |
| 4 | 0.94 | 0.72 | 0.48 | 0.46 | 0.50 | 976 | flat ≈ 50 % |
| 5 | 0.94 | 0.78 | 0.76 | 0.60 | 0.78 | 875 | ≈ 60–78 % |
| 6 | 0.98 | 0.70 | 0.60 | 0.58 | 0.62 | 788 | ≈ 60 % |
| 7 | 1.00 | 0.94 | 0.82 | 0.82 | 0.80 | 571 | ≈ 80 % |
| 9 | 1.00 | 0.98 | 0.86 | 0.88 | 0.90 | 449 | ≈ 88 % |
| 10 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 | 0 | dead (SSB-filter failure, see item 2b) |

Reading: no level clears FER ≤ 10 % with a Wilson bound at any SNR; the floors are SNR-independent from ≈ 18 dB, i.e. pure fading outage that more SNR cannot buy back (the fade-diversity gap, item 3). The best *goodput* on Poor at any SNR is ≈ 0.9–1.0 kbps PHY (levels 3–4 at ≈ 25–50 % FER) — before ARQ retransmission cost and before turnaround, versus VARA-2300's ≈ 1.5 kbps and PACTOR-4's ≈ 2.9 kbps *net delivered* at 20 dB MPP in the IONOS study, both with zero link drops. Below 12 dB, sync detection (not FEC) is the dominant failure at every level (12–17 of 50 sync failures at 6 dB) — item 20's unfiltered-detector margin is real.

**CCIR Moderate (1 ms / 0.5 Hz) + SSB filter, `hf_standard` every level** (`results/review-2026-09-05/mod-ssb-std/moderate.csv`). FER per SNR; (sync / LDPC) failures out of 50:

| Level | 6 dB | 12 dB | 18 dB | 24 dB | 30 dB | Peak goodput | Floor character |
|---|---|---|---|---|---|---|---|
| 1 | 0.22 (11/0) | 0.12 (6/0) | 0.02 (1/0) | 0.00 | 0.04 | 328 bps | clean from 18 dB; sync-limited below |
| 2 | 0.30 (11/4) | 0.18 (6/3) | 0.08 (0/4) | 0.06 | 0.08 | 645 | ≈ 6–8 % LDPC floor (50 trials — cannot clear a Wilson 10 % bound) |
| 3 | 0.56 | 0.42 | 0.12 | 0.16 | 0.14 | 1046 | ≈ 12–16 % |
| 4 | 0.86 | 0.64 | 0.34 | 0.42 | 0.42 | 1193 | ≈ 34–42 % |
| 5 | 0.94 | 0.78 | 0.72 | 0.52 | 0.74 | 1051 | ≈ 50–75 % |
| 6 | 1.00 | 0.78 | 0.60 | 0.56 | 0.62 | 826 | ≈ 60 % |
| 7 | 1.00 | 0.96 | 0.92 | 0.86 | 0.84 | 457 | ≈ 85 % |
| 9 | 1.00 | 0.98 | 0.94 | 0.96 | 0.98 | 193 | ≈ 95 % |
| 10 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 | 0 | dead (item 2b) |

Reading: on the CCIR-Moderate channel a real SSB rig should expect — the ARRL/Winlink "typical" channel — coppa's usable ladder is levels 1–3 (BPSK/QPSK 1/2, ≈ 0.3–1.0 kbps PHY at ≤ 16 % FER) from 18 dB; everything from QPSK 3/4 up sits on a 35–100 % SNR-independent outage floor. Best PHY goodput at any SNR on Moderate is ≈ 1.2 kbps (L4 at 34 % FER), against VARA-2300 MPG ≈ 1.6 kbps and PACTOR-4 ≈ 2.9 kbps *net* at 20 dB with no drops. Failure mode is LDPC non-convergence at every level from 18 dB up (a per-frame fade the code/interleaver cannot cover — item 3), and preamble sync below 12 dB (item 20).

Raw CSVs for all fresh runs: `results/review-2026-09-05/{awgn-ssb-std,awgn-ssb-std-hi,awgn-nossb-std-hi,poor-ssb-std,mod-ssb-std}/`.
