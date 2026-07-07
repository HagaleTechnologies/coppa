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
- PAPR clipping uses per-speed-level targets (6.0 dB at BPSK up to 14.0 dB at 64QAM 7/8, tuned in `SPEED_LEVELS`); the old flat/too-aggressive clipping this line used to describe was fixed well before Phase 1. The remaining rough edge is levels 9/10 (64-QAM) hitting LDPC non-convergence at high SNR in `crates/coppa/tests/phase_c_loopback.rs` — a decoder/code-rate issue, not a PAPR-clipping one
- Daemon hardware audio requires the `cpal-backend` feature; without it the daemon runs but moves no audio
- WebSocket server lacks integration tests
- Channel adaptation is EWMA-only; there is no ML model loading or inference (the `coppa-ml` model registry always falls back to EWMA)
- `coppa-channel` models AWGN + a two-tap Watterson/ITU-R F.1487 HF channel (Rayleigh taps, Gaussian Doppler) plus an `ssb_filter` helper emulating a realistic 300-2700 Hz SSB rig audio passband. The sinusoidal `fading()` helper is AGC-test-only.
- The waveform occupies a realistic ~300-2700 Hz SSB passband (carrier offset + in-band Newman-phase preamble) with TX section leveling/bandpass conditioning and a streaming O(1) preamble sync detector (`SyncDetector`, ~0.0015-0.0035x realtime) — see `docs/adr/003-phase1-waveform-break.md`. This is a wire-format break from earlier waveform revisions; old and new are not interoperable
- **Watterson-fading regression on sparse-pilot HF profiles (`hf_standard`/`hf_wide`/`hf_narrow`, levels 1-4) — FIXED for levels 1-3, partially fixed for level 4.** Bisected to Phase 1 Task 5's `SyncDetector` anchoring sync timing on the first-arriving multipath tap rather than the strongest one; fixed by preferring the strongest tap unless it's more than half a cyclic prefix away from the first arrival (preserving the original anti-echo safety intent for delay spreads beyond anything this codebase's Watterson presets model). Levels 1-2 now match or exceed pre-Phase-1 Watterson-Moderate/Poor performance; level 3 is very close (within normal trial variance); level 4 (QPSK 3/4) improves substantially (peak goodput up from ~330-630 bps to ~555-1234 bps) but retains a real, smaller residual gap (72-76% of pre-Phase-1 peak goodput), not yet investigated further. See `docs/adr/004-strongest-path-timing.md`, BENCHMARKS.md's "2026-07 — Hotfix: sparse-pilot header Watterson-fading regression" section, and `.superpowers/sdd/p1-fading-regression-fix-report.md`
