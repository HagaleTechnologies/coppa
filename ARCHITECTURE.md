# Coppa Architecture & Project Plan

## Executive Summary

Coppa is an open-source ham radio digital communications system written in Rust, published as a **reference implementation** of an OFDM HF modem (DSP + FEC + protocol). It prioritizes clarity, correctness, and maintainability. Coppa is not a finished on-air product and does not interoperate with existing proprietary modes such as VARA; its value is a clean, well-tested, readable Rust implementation of the underlying techniques (OFDM, LDPC, Viterbi, ARQ).

## Core Design Principles

1. **Modular Architecture**: Each protocol and modem type is a separate crate
2. **Clean Interfaces**: Well-defined trait boundaries between layers
3. **Incremental Development**: Start small, prove concepts, then expand
4. **Cross-Platform**: Works on Linux, macOS, Windows, and embedded systems
5. **Performance**: Optimized DSP pipeline for real-time operation

## System Architecture

### Layer Overview

```
Application Layer    → CLI (coppa), Daemon (coppad), C FFI
Host Interface       → VARA TCP server, WebSocket JSON API
Engine Layer         → Core orchestration, profiles, configuration
Protocol Layer       → Framing, FEC, ARQ, compression, session mgmt
Codec Layer          → BPSK, QPSK, 8PSK, QAM, OFDM modulation/demod
DSP Layer            → FFT, filters, AGC, resampling, signal processing
Hardware Layer       → Audio I/O (CPAL/WAV), radio control, PTT
Adaptation Layer     → Channel prediction (EWMA), MCS selection, spectrum sensing
Test Infrastructure  → Channel models (AWGN, fading), fuzzing
```

### Component Responsibilities

#### DSP Primitives (`coppa-dsp`)
- FFT/IFFT via rustfft
- FIR and IIR digital filters
- Automatic Gain Control (AGC)
- Sample rate conversion
- Signal generation and analysis

#### Codec Layer (`coppa-codec`)
- **BPSK**: RRC pulse-shaped with Costas loop carrier recovery
- **QPSK/8PSK/16QAM/64QAM**: Constellation mappers only (no integrated modem yet)
- **OFDM**: Multi-carrier modulation with configurable profiles, Hermitian symmetry, cyclic prefix, pilot-based channel estimation, MMSE equalization
- **Traits**: `Modem`, `FecCodec`, `ChannelEstimator` defined as intended extension points (not all are wired through the flagship modems yet)

#### Protocol Layer (`coppa-protocol`)
- **Framing**: V1 and V2 PHY frames with preamble, sync word, CRC-16
- **FEC**: rate-1/2 K=7 convolutional code with soft Viterbi decoder; QC-LDPC codes at 6 rates (1/4, 1/3, 1/2, 2/3, 3/4, 7/8)
- **ARQ**: Selective repeat with configurable window size
- **Compression**: Fixed Huffman table optimized for ham radio text, LZ4 for bulk data
- **Session**: Connection state machine, callsign management

#### Channel Models (`coppa-channel`)
- AWGN noise at configurable SNR
- Deterministic sinusoidal amplitude fade (a simple test impairment — **not** Rayleigh/Watterson fading and not a realistic HF channel model)
- Frequency offset generation (note: the OFDM sync path does not yet correct CFO)
- Used for testing only, not shipped in production

#### Audio I/O (`coppa-audio`)
- **CPAL backend**: Real-time audio capture and playback via ring buffers (rtrb)
- **File backend**: WAV read/write via hound
- Device enumeration and selection

#### Radio Control (`coppa-radio`)
- **rigctld**: TCP client for hamlib CAT control (frequency, mode, PTT)
- **Serial PTT**: DTR/RTS control line toggling (stub — requires serialport crate and hardware)
- **GPIO PTT**: Linux sysfs GPIO pin control (stub — requires hardware)

#### Channel Adaptation (`coppa-ml`)
- `ChannelPredictor` trait with an EWMA + linear-trend predictor (the only predictor implemented — there is no machine learning or model inference)
- An optional model registry that scans for model files and always falls back to the EWMA predictor (no inference runtime is integrated)
- MCS selection from a static SNR-threshold lookup table
- Spectrum sensing utilities

#### Engine (`coppa-engine`)
- `CoppaCore`: integrates modem, FEC, and framing into encode/decode pipelines
- Operating profiles (HF_ROBUST, HF_STANDARD, VHF_FAST, etc.)
- Configuration management

#### Host Interfaces (`coppa-host`)
- **VARA-style TCP**: command port (8300) + data port (8301) control server, VARA-inspired wire format (not bit-compatible with VARA modem signals; does not interoperate with VARA over the air)
- **WebSocket**: JSON API for web-based clients (types defined, server scaffolded)

#### C FFI (`coppa-ffi`)
- `coppa_engine_create/destroy`, `coppa_encode/decode` for one-shot operations
- Streaming API: `coppa_start_stream`, `coppa_feed_samples`, `coppa_get_decoded`
- cdylib + staticlib output for C, Python, Swift integration

#### CLI (`coppa-cli`)
- Subcommands: `tx`, `rx`, `loopback`, `listen`, `devices`, `config`
- WAV file I/O via file-backend feature

#### Daemon (`coppa-daemon`)
- Tokio-based event loop multiplexing audio, host, and radio events
- Configuration via TOML file
- Ctrl-C graceful shutdown

## TX Pipeline

```
Text → Frame (preamble + sync + length + data + CRC)
     → FEC encode payload (rate-1/2 K=7 convolutional code)
     → Baseband NRZ (±1)
     → RRC pulse shaping (α=0.35, 4-symbol span)
     → Upconvert to 1 kHz carrier
     → Audio samples
```

## RX Pipeline

```
Audio samples
  → Adaptive AGC (block-adaptive, attack/release asymmetry)
  → Costas loop (carrier recovery, handles ±30 Hz offset)
  → RRC matched filter (completes raised cosine, rejects mixer products)
  → Eye-opening timing recovery (optimal sampling phase search)
  → Sync word detection (with preamble validation + polarity resolution)
  → Viterbi soft-decision FEC decoding
  → Frame parse + CRC verification
  → Text
```

## Current Status

- 12-crate workspace, ~26,000 lines of code, 600+ tests
- Full BPSK loopback verified under: clean, AWGN, amplitude variation, and a sinusoidal amplitude fade (no carrier-frequency-offset tolerance — see Current Limitations)
- OFDM modulator/demodulator with multiple profiles (HF narrow/standard, VHF wide)
- LDPC encoder/decoder at 6 code rates with belief propagation
- Audio I/O via CPAL with ring buffer architecture
- VARA-style TCP control interface (not RF-compatible with VARA)
- C FFI with streaming decode API
- Property-based testing and fuzzing infrastructure
- CI with check, test, clippy, and format jobs

## Current Limitations

- **OFDM decode not wired**: OFDM modulator works but full OFDM framing/decode pipeline is not integrated into the engine
- **LDPC/ARQ/session/compression implemented but not wired**: These subsystems work in isolation but are not yet integrated into the encode/decode data path
- **Serial/GPIO PTT are stubs**: Track state but do not access hardware (require serialport crate / sysfs)
- **ML model loading**: Always falls back to EWMA predictor (no trained ONNX models)
- **Text-only payloads**: `CoppaCore::encode/decode` accepts/returns UTF-8 text only; binary payloads need `encode_bytes/decode_bytes` (not yet implemented)

## Dependencies

```toml
# Core
anyhow = "1.0"          # Error handling
num-complex = "0.4"     # Complex number arithmetic
rustfft = "6.2"         # FFT/IFFT
crc = "3.2"             # CRC-16 checksums
smallvec = "1.13"       # Stack-allocated small vectors

# Audio
cpal = "0.15"           # Cross-platform audio I/O
hound = "3.5"           # WAV file read/write
rtrb = "0.3"            # Real-time ring buffer

# Protocol
lz4_flex = "0.11"       # LZ4 compression

# Async / Networking
tokio = "1"             # Async runtime
serde = "1"             # Serialization
serde_json = "1"        # JSON for WebSocket API
toml = "0.8"            # Configuration files

# CLI
clap = "4"              # Command-line argument parsing

# Testing
rand = "0.9"            # Random number generation
proptest = "1.5"        # Property-based testing
criterion = "0.5"       # Benchmarking
approx = "0.5"          # Floating-point comparisons
```

## Project Structure

```
coppa/
├── Cargo.toml                    # Workspace root
├── README.md                     # Project overview
├── ARCHITECTURE.md               # This document
├── crates/
│   ├── coppa-dsp/                # Pure DSP primitives
│   ├── coppa-codec/              # Modulation/demodulation
│   │   └── src/ofdm/            # OFDM subsystem (sync, equalizer, pilots, frames)
│   ├── coppa-protocol/           # Framing, FEC, ARQ, compression
│   │   └── src/fec/ldpc/        # LDPC encoder, decoder, code definitions
│   ├── coppa-channel/            # Channel models for testing
│   ├── coppa-audio/              # Audio I/O backends
│   ├── coppa-radio/              # Radio control (rigctld, serial PTT, GPIO PTT)
│   ├── coppa-ml/                 # Channel prediction (EWMA) + MCS selection
│   ├── coppa-engine/             # Core engine
│   ├── coppa-host/               # Host interfaces (VARA TCP, WebSocket)
│   ├── coppa-ffi/                # C FFI bindings
│   ├── coppa-cli/                # CLI binary (coppa)
│   └── coppa-daemon/             # Daemon binary (coppad)
├── tests/                        # Workspace integration tests
├── benches/                      # Criterion benchmarks
└── fuzz/                         # Fuzzing targets
```

## Development Roadmap

### Completed
- [x] BPSK modem with full DSP chain
- [x] Frame structure with CRC-16
- [x] FEC: convolutional code with soft Viterbi decoder
- [x] OFDM modulator/demodulator with profiles
- [x] LDPC encoder/decoder (6 rates)
- [x] QPSK, 8PSK, 16QAM, 64QAM constellation mappers
- [x] ARQ with selective repeat
- [x] Huffman + LZ4 compression
- [x] Audio I/O via CPAL
- [x] VARA-style TCP control interface
- [x] C FFI with streaming API
- [x] CLI with tx/rx/loopback commands
- [x] Daemon with event loop
- [x] ML channel predictor (EWMA)
- [x] CI pipeline

### Not implemented (out of scope for the reference implementation)
- [ ] Carrier-frequency-offset (CFO) correction in the OFDM sync path
- [ ] Realistic HF channel model (Rayleigh/Watterson) for testing
- [ ] Symbol clock recovery for TX/RX sample rate mismatch
- [ ] Serial/GPIO PTT hardware integration
- [ ] Runtime modulation switching / higher-order QAM wired end-to-end (PAPR handling for the highest modes is incomplete)
- [ ] Speed negotiation in ARQ

## Goals

These are project goals, not validated results. Coppa has no measured on-air or
standardized-channel (e.g. Watterson) performance data, and makes no throughput or
bit-error-rate claims.

1. **Clarity**: readable, well-documented reference code for OFDM, LDPC, Viterbi, and ARQ
2. **Correctness**: high unit-test coverage of the DSP/FEC building blocks
3. **Portability**: builds on Linux, macOS, and Windows
4. **Efficiency (goal)**: real-time processing on modest hardware such as a Raspberry Pi 4
