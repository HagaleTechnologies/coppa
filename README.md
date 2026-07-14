# Coppa

Open-source ham radio digital communications system written in Rust.

[![CI](https://github.com/HagaleTechnologies/coppa/actions/workflows/ci.yml/badge.svg)](https://github.com/HagaleTechnologies/coppa/actions)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)

## Status

| Component | Status | Notes |
|-----------|--------|-------|
| BPSK modem (loopback) | **Working** | Full DSP chain: AGC, Costas loop, RRC, timing recovery |
| BPSK modem (live audio) | **Working** | Requires `--features cpal-backend` |
| Convolutional FEC | **Working** | K=7 rate-1/2 with soft Viterbi decoder |
| Frame sync + CRC | **Working** | Handles 180-degree phase ambiguity |
| QPSK/8PSK/QAM mappers | **Working** | Constellation mapping/demapping only (not wired to engine) |
| OFDM mod/demod | **Partial** | Round-trip works in isolation; sync has CFO limitations |
| LDPC codes | **Working** | 6 rates; encode/decode verified |
| ARQ + protocol stack | **Working** | Selective repeat, session management, compression |
| CLI (loopback, tx, rx) | **Working** | File-based I/O; live audio requires feature flag |
| Daemon event loop | **Working** | Processes audio and host events |
| VARA-style TCP interface | **Working** | Control/data server on ports 8300/8301; VARA-style protocol, not RF-compatible with VARA |
| C FFI | **Working** | One-shot and streaming encode/decode with panic safety |
| Operating profiles | **Partial** | Defined but only HF modes use BPSK; no runtime modulation switching |
| Channel prediction | **Working** | EWMA predictor + static MCS lookup table (no machine learning / model inference) |
| Serial/GPIO PTT | **Stub** | Feature flags exist but no hardware access |
| VARA RF/waveform compat | **Not implemented** | The TCP control interface is VARA-style, but the modem is not RF-compatible with VARA and does not interoperate with it |

## Features

- **Full DSP chain**: AGC, Costas loop carrier recovery, RRC pulse shaping, Gardner timing recovery
- **BPSK modem**: end-to-end encode/decode with convolutional FEC
- **Modulation**: BPSK is wired end-to-end; QPSK, 8PSK, 16QAM, 64QAM, and OFDM constellation math is implemented and tested but not wired into the engine pipeline
- **Forward error correction**: rate-1/2 K=7 convolutional codes with soft Viterbi, QC-LDPC (6 rates)
- **OFDM PHY**: configurable profiles, pilot-based channel estimation, MMSE equalization
- **Protocol layer**: ARQ with selective repeat, Huffman + LZ4 compression, CRC-16 framing
- **Audio I/O**: real-time via CPAL (feature-gated), WAV file read/write
- **VARA-style TCP interface**: command/data port server for host application integration (not RF-compatible with VARA)
- **C FFI**: shared/static library with streaming decode API
- **Channel prediction**: EWMA-based with MCS selection table

## Quick Start

```bash
cargo build --workspace
cargo test --workspace
```

## CLI Examples

```bash
# Loopback test (encode then decode in memory)
coppa loopback "Hello from Coppa"

# Encode a message to a WAV file
coppa tx "CQ CQ CQ DE VK2ABC K" -o cq.wav

# Decode a WAV file
coppa rx -i cq.wav

# List audio devices (requires cpal-backend feature)
coppa devices

# Show available operating profiles
coppa config
```

## Workspace Crates

| Crate | Description |
|-------|-------------|
| `coppa-dsp` | Pure DSP primitives: FFT/IFFT, FIR/IIR filters, AGC, resampling |
| `coppa-codec` | Modulation/demodulation: BPSK, QPSK, 8PSK, QAM, OFDM |
| `coppa-protocol` | Framing, FEC (convolutional + LDPC), ARQ, compression, session management |
| `coppa-channel` | Channel models for testing: AWGN, sinusoidal fading, frequency offset |
| `coppa-audio` | Audio backends: CPAL real-time I/O, WAV file read/write |
| `coppa-radio` | Radio control: rigctld CAT, serial PTT (DTR/RTS, `serial-ptt` feature), GPIO PTT (Linux sysfs, `gpio-ptt` feature) |
| `coppa-ml` | Channel prediction: EWMA predictor, MCS selection, spectrum sensing |
| `coppa-engine` | Core engine orchestrating modem, FEC, and framing pipelines |
| `coppa-host` | Host interfaces: VARA-style TCP control server, WebSocket JSON API |
| `coppa-ffi` | C FFI bindings (cdylib + staticlib) with streaming decode API |
| `coppa-cli` | CLI binary (`coppa`) for transmit, receive, loopback, device listing |
| `coppa-daemon` | Long-running daemon (`coppad`) with event loop, audio, and host integration |

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full system design, TX/RX pipeline details, and layer responsibilities.

See [PLAN-hardening.md](PLAN-hardening.md) for known issues and the hardening roadmap.

See [docs/SPEC.md](docs/SPEC.md) for the normative waveform/FEC/protocol conformance specification — the reference for building an independent, interoperable implementation.

See [docs/OPERATING.md](docs/OPERATING.md) for field-operation guidance, including TX level calibration (`coppa tune`).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

**MSRV**: 1.85.0
