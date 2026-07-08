# Coppa - Claude Code Instructions

## Project Overview

Coppa is an open-source OFDM digital modem for amateur radio, written in Rust and published as a **reference implementation** of an HF modem's DSP/FEC/protocol stack. It includes a full DSP chain, a protocol stack with ARQ, an AFSK 1200/AX.25 TNC, CLI tools, a daemon, C FFI bindings, and a VARA-style TCP control interface (modeled on VARA's TCP TNC API; the modem is **not** RF/waveform-compatible with VARA and does not interoperate with it).

## Build & Test Commands

```bash
# Build
cargo build --workspace

# Fast tests (lib-only, used in CI)
cargo test --workspace --lib

# Full test suite (includes integration + proptest — run before pushing)
cargo test --workspace

# With feature flags
cargo test --workspace --features cpal-backend,websocket --lib

# Clippy (CI runs with -D warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt --all -- --check

# Benchmarks
cargo bench --workspace
```

## Testing Policy

**Run `cargo test --workspace` locally before pushing for fast feedback.** The full test suite (integration tests, proptest roundtrips) now runs in CI on every push/PR (`test-full` job, Linux, `--features cpal-backend,websocket`). Local full-suite runs are still recommended before pushing — CI catches it, but local runs are faster feedback. At minimum, run `cargo test --workspace --lib` for a quick sanity check.

## Workspace Structure

12 crates under `crates/`:

| Crate | Role |
|-------|------|
| `coppa-dsp` | Pure DSP: FFT, filters, AGC, resampling |
| `coppa-codec` | Modulation: BPSK, QPSK, 8PSK, QAM, OFDM |
| `coppa-protocol` | Framing, FEC (convolutional + LDPC), ARQ, compression, sessions |
| `coppa-channel` | Channel models for testing (AWGN, fading, CFO) |
| `coppa-audio` | Audio backends: CPAL (feature-gated), WAV file I/O |
| `coppa-radio` | Radio control via rigctld CAT |
| `coppa-ml` | Channel prediction, MCS selection |
| `coppa-engine` | Core engine: thin wrapper around CoppaTransceiver |
| `coppa-host` | VARA-style TCP control server, WebSocket JSON API |
| `coppa-ffi` | C FFI (cdylib + staticlib) with streaming decode |
| `coppa-cli` | CLI binary (`coppa`) |
| `coppa-daemon` | Daemon binary (`coppad`) |

## Key Architecture

- **CoppaTransceiver** (in `coppa-protocol`) composes CoppaModem + LDPC + constellation mappers + block interleaver. This is the main encode/decode pipeline.
- **CoppaCore** (in `coppa-engine`) is a thin ~210-line wrapper around CoppaTransceiver.
- **9 speed levels** replace old mcs_index/fec_rate/modulation config. All profiles unified at 48kHz sample rate.
- FFI uses pointer-to-pointer semantics in `coppa_engine_destroy` to prevent double-free.

## CI

Single workflow at `.github/workflows/ci.yml` runs on push/PR to main:
- `cargo check`, `cargo test --lib` (fast signal, with features), **full test suite** (`cargo test --workspace --features cpal-backend,websocket`, Linux), clippy, fmt, MSRV (1.85.0), platform checks (Linux/macOS/Windows, `--lib` only on non-Linux to save runner minutes), **cargo-deny** supply-chain check, security audit (rustsec/audit-check).

## MSRV

1.85.0 (enforced in CI and `Cargo.toml`).

## Known Limitations

- CFO (carrier frequency offset) tolerance is ±50 Hz via two-stage acquisition (coarse Moose + fine Schmidl-Cox, resolved through their ambiguity periods), not unlimited — beyond that the ambiguity resolution itself wraps, and sample-clock offset is still uncorrected
- PAPR clipping uses per-speed-level targets (6.0 dB at BPSK up to 14.0 dB at 64QAM, tuned in `SPEED_LEVELS`); the old flat/too-aggressive clipping this line used to describe was fixed well before Phase 1. **The levels 9/10 (64-QAM) LDPC-non-convergence-at-high-SNR issue this bullet used to describe is FIXED** by Task 4's NR BG2 mother code + level 10's rate-7/8→5/6 change: `tests/phase_c_loopback.rs`'s `test_snr_fer_monte_carlo` now shows FER=0.00/100 for every level (1-10) across its whole swept SNR range, confirmed by a fresh run, not carried over from an old measurement
- The Phase 2 Task 4 alpha-calibration process itself is a cautionary tale worth keeping in mind for future LDPC-parameter tuning: a normalized-min-sum scale picked from a sweep at **one** speed level (level 2) measurably improved that level's isolated FER but broke real convergence (100% frame loss, even on a clean channel) for level 10's very different operating point (highest rate, least redundancy, heaviest known-pad pinning at small payloads) — caught by `tests/phase_c_loopback.rs`'s existing tests, not by the new codec's own unit tests, which didn't happen to exercise that combination. Any future alpha/decoder-parameter change should be validated across the *whole* speed ladder (and payload-size extremes), not a single representative level, before being adopted as the shipped default
- Daemon hardware audio requires the `cpal-backend` feature; without it the daemon runs but moves no audio
- WebSocket server lacks integration tests
- Channel adaptation is EWMA-only; there is no ML model loading or inference (the `coppa-ml` model registry always falls back to EWMA)
- `coppa-channel` models AWGN + a two-tap Watterson/ITU-R F.1487 HF channel (Rayleigh taps, Gaussian Doppler) plus an `ssb_filter` helper emulating a realistic 300-2700 Hz SSB rig audio passband. The sinusoidal `fading()` helper is AGC-test-only.
- The waveform occupies a realistic ~300-2700 Hz SSB passband (carrier offset + in-band Newman-phase preamble) with TX section leveling/bandpass conditioning and a streaming O(1) preamble sync detector (`SyncDetector`, ~0.0015-0.0035x realtime) — see `docs/adr/003-phase1-waveform-break.md`. This is a wire-format break from earlier waveform revisions; old and new are not interoperable
- The LDPC layer is a single 5G-NR-style BG2 mother code (Zc=176 lifting, `crate::fec::ldpc::NrLdpc`) shared by every speed level, rate-matched down per level via a circular buffer (`rate_match.rs`) instead of switching between nine separate per-rate 802.11 QC-LDPC codes — see `docs/adr/005-nr-bg2-ldpc.md`. Level 10's nominal code rate moved from 7/8 to **5/6** as part of this change (the audited `k_used` table). This is a wire-format break: frames encoded with the pre-this-change codec are not decodable by the new one, and vice versa, old and new are not interoperable — same pattern as the Phase 1 waveform break above. Two measured, currently-unmet gaps from that change's own acceptance targets, kept open rather than hidden: (1) the coding gain at matched rate/block-length is real but smaller than the density-evolution-based prediction (measured +0.5 dB at level 2 isolated-FEC-layer AWGN vs. a predicted ~1.8 dB; a direct layered-vs-flooding A/B on identical trials ruled out a decoder-schedule bug, so this is believed to be a real finite-length effect, not investigated further); (2) decode CPU/frame is worse than the accepted ≤3x budget across the whole ladder (3.5x-9.5x measured after a real, verified ~19% optimization of the layered update's hot loop) because the shared mother code's graph (42 base rows, Zc=176) no longer shrinks for high-rate levels the way the old per-rate codes' graphs did — closing this would need a materially larger effort (SIMD, unsafe bounds-check elision, cache-aware graph relabeling), not attempted here. See `.superpowers/sdd/p2-task-4-report.md` for the full investigation
- The frame header is soft-decoded (per-bit LLRs, full 4096-codeword Golay ML + CRC-assisted list-2, Phase 2 Task 2), a clean, verified improvement over the old hard-decision header — but the plan's original acceptance figure ("≥25 percentage points header-decode-rate gain, soft vs. hard, at 200 seeds of watterson-poor, 8 dB") was not achievable and was honestly re-derived: Task 1/7's estimation work (already merged before Task 2 started) had already made hard-decision header decoding fairly robust at 8 dB, leaving only 5-7 points of headroom. The real, reproducible gap is 6-8 percentage points at a lower SNR (~3 dB), before sync itself becomes the binding constraint
- Known-pad LLR pinning and exact max-log LLR scaling (Phase 2 Task 3) measure a real +3.0 dB gain at the FEC-isolated layer, but this gain is **invisible in a full end-to-end OFDM bench** at some operating points (e.g. `hf_standard`/level 2/short payload/AWGN) because OFDM sync, not LDPC convergence, is the sole binding constraint there — every measured frame failure at that operating point was `SyncFailed`/`HeaderCorrupt`, never `LdpcNotConverged`. Any future payload-side FEC improvement will not show up in end-to-end goodput/FER benchmarks until sync's own SNR floor is at or below the LDPC decode's floor — verify FEC-layer gains with an isolated (OFDM-bypassing) bench, not just the full pipeline
- **Watterson-fading regression on sparse-pilot HF profiles (`hf_standard`/`hf_wide`/`hf_narrow`, levels 1-4) — FIXED for levels 1-3, partially fixed for level 4.** Bisected to Phase 1 Task 5's `SyncDetector` anchoring sync timing on the first-arriving multipath tap rather than the strongest one; fixed by preferring the strongest tap unless it's more than half a cyclic prefix away from the first arrival (preserving the original anti-echo safety intent for delay spreads beyond anything this codebase's Watterson presets model). Levels 1-2 now match or exceed pre-Phase-1 Watterson-Moderate/Poor performance; level 3 is very close (within normal trial variance); level 4 (QPSK 3/4) improves substantially (peak goodput up from ~330-630 bps to ~555-1234 bps) but retains a real, smaller residual gap (72-76% of pre-Phase-1 peak goodput), not yet investigated further. See `docs/adr/004-strongest-path-timing.md`, BENCHMARKS.md's "2026-07 — Hotfix: sparse-pilot header Watterson-fading regression" section, and `.superpowers/sdd/p1-fading-regression-fix-report.md`
- **Phase 2 channel estimation (Task 1 delay-domain estimator + Task 7 Kalman tracker) — a real, unresolved regression, shipped anyway.** Replacing `LinearInterpolationEstimator` with a delay-domain ridge-LS estimator (`crate::ofdm::delay_domain::DelayDomainEstimator`) regressed Watterson-Moderate/level 2's FER≤10% threshold from 18 dB to 24 dB (needed ≥1.5 dB *better*), root-caused to the estimator's frame-global coarse-delay reference not tracking real intra-frame drift (a per-window adaptive fix was built, tested, and reverted — it improved raw channel-estimate accuracy but let occasional low-SNR windows corrupt the LDPC-facing noise variance, making full-sweep FER *worse*). A Kalman/RTS tracker (`crate::ofdm::kalman_tracker::KalmanLagSmoother`) was pulled forward specifically to close this gap; it fixed one real bug (overlapping pooled-pilot windows double-counted as independent evidence) but a systematic sweep of its AR(1) forgetting coefficient across almost two orders of magnitude showed near-total FER flatness, and the final measured threshold is **30 dB** — worse than both the pre-Phase-2 baseline and Task 1's own regressed number. Believed to be a model-class mismatch (the intra-frame drift looks more like an accumulating phase/coarse-delay reference error than stationary Rayleigh amplitude fading, which an AR(1) tap-amplitude model doesn't represent), not a tuning problem — not fixed in Phase 2. See `docs/adr/006-phase2-parametric-estimation-nr-bg2.md` (decisions 1 and 3) and `.superpowers/sdd/p2-task-1-report.md`/`p2-task-7-report.md`
- **Turbo re-estimation (Task 5) rescues frames on decode failure, but the benefit is heavily concentrated on low-order modulation.** One-round LDPC-aided re-estimation (soft virtual pilots from posterior LLRs, re-fit/re-demap/re-decode) rescues 21-50% of first-pass failures on level 2 (BPSK) under Watterson-Moderate/Poor (+6.0 dB at FER≤10% on Moderate), but only 1-2% on levels 5/6 (8PSK/16QAM) under Watterson-Poor. Plausibly connected to the still-open Task 1/7 estimation issue above (a first pass with overconfident wrong LLRs seeds virtual pilots with backwards-weighted confidence) but not confirmed — see `.superpowers/sdd/p2-task-5-report.md`
- **16-QAM's fast soft demapper (Task 6) is allocation-bound, not arithmetic-bound.** The closed-form per-axis min-reduction replacement for `Qam16Mapper`/`Qam64Mapper::demap_soft` is 19-162x faster in raw arithmetic (verified against a brute-force oracle), and 64-QAM's full production-API call clears a ≥8x speedup target (28-34x). 16-QAM's full-API call only reaches 4.2-4.3x: both the old and new code pay an identical, fixed `Vec<f32>` heap-allocation cost (via the shared `ConstellationMapper` trait) that dominates at 16-QAM's smaller workload (4 bits × 16 points) — not something the closed-form replacement itself can fix without a cross-cutting trait-signature change affecting every modulation in `coppa-codec`
- **Phase 2's cumulative full-ladder re-baseline (Task 8) does not cleanly clear the phase's own acceptance bar.** AWGN is met and exceeded (level 4 +3 dB, level 7 +3 dB, level 9 +6 dB at FER≤10%; level 10 fixed from non-convergent to clearing cleanly), and the soft header's failure share on Poor is met in aggregate (4.3%). But watterson-poor/level 2's "≥+3 dB" bar and watterson-poor/level 6's "≥+1.5 dB" bar are both **not measurable as literally specified** — Poor is an irreducible-outage-floor channel where neither the pre- nor post-Phase-2 codec ever crosses 10% FER at either level. The underlying (non-threshold) FER curves show level 2 substantially better at every SNR (roughly halved at 30 dB), a real win the literal metric can't express, but level 6 shows no such gain (flat to marginally worse). Watterson-Moderate is a genuinely mixed cumulative result: levels 1-2 (BPSK) improve at FER≤10% (turbo re-estimation's concentrated benefit outweighing the estimator regression), but levels 3-6 (QPSK, 8PSK, 16QAM 1/2) show a **real regression at matched SNR** (e.g. level 6 at 30 dB: 10.25%→40.0% FER), while levels 7/9 (16QAM 3/4, 64QAM 2/3) improve consistently. A previously-unexercised CFO×level-4 interaction also surfaced: level 4 (QPSK 3/4) under a 40 Hz carrier offset goes from clearing FER≤10%/≤1% to never clearing at all (peak goodput −46%) — not caught by any single dev task's own bench gate. See `BENCHMARKS.md`'s "2026-07 — Phase 2 (parametric estimation + NR BG2): cumulative re-baseline" section for the complete per-level, per-channel data, and `docs/adr/006-phase2-parametric-estimation-nr-bg2.md` for the full decision record

## Knowledge wiki

`wiki/INDEX.md` is the map of accumulated knowledge — read it before deep
exploration; open pages relevant to your task. After substantive work, run
/wiki-update: distill new gotchas/decisions/corrections into the wiki (or
into docs/ if normative — the wiki points, it never restates). The wiki is
descriptive and always loses conflicts with code and docs/.

## Multi-agent hygiene

You are never alone in this repo — other agents may be working concurrently
in other clones, branches, or worktrees.

- **Start fresh:** `git fetch` and rebase onto `origin/main` before reading
  code or making decisions; stale context produces wrong work.
- **Claim before work:** search open PRs/issues first; open a draft PR early —
  the draft PR *is* the claim. Don't duplicate in-flight work.
- **Isolate:** always a branch (worktree preferred), never a shared checkout's
  main. Use per-session scratch dirs; don't bind fixed ports.
- **Flush at the end:** push (`--force-with-lease` only) and open/update your
  PR before finishing. Unpushed work is invisible work.
- **Main moves only by PR merge.**
