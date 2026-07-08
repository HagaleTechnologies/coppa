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
- **Watterson-fading regression on sparse-pilot HF profiles (`hf_standard`/`hf_wide`/`hf_narrow`, levels 1-4) — FIXED for levels 1-3, partially fixed for level 4.** Bisected to Phase 1 Task 5's `SyncDetector` anchoring sync timing on the first-arriving multipath tap rather than the strongest one; fixed by preferring the strongest tap unless it's more than half a cyclic prefix away from the first arrival (preserving the original anti-echo safety intent for delay spreads beyond anything this codebase's Watterson presets model). Levels 1-2 now match or exceed pre-Phase-1 Watterson-Moderate/Poor performance; level 3 is very close (within normal trial variance); level 4 (QPSK 3/4) improves substantially (peak goodput up from ~330-630 bps to ~555-1234 bps) but retains a real, smaller residual gap (72-76% of pre-Phase-1 peak goodput), not yet investigated further. See `docs/adr/004-strongest-path-timing.md`, BENCHMARKS.md's "2026-07 — Hotfix: sparse-pilot header Watterson-fading regression" section, and `.superpowers/sdd/p1-fading-regression-fix-report.md`

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
