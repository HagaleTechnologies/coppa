# Coppa

Open-source ham radio digital communications system written in Rust.

[![CI](https://github.com/HagaleTechnologies/coppa/actions/workflows/ci.yml/badge.svg)](https://github.com/HagaleTechnologies/coppa/actions)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)

## Status

| Component | Status | Notes |
|-----------|--------|-------|
| OFDM modem | Implemented | Engine encode/decode and streaming receive use the OFDM transceiver; carrier-frequency-offset tolerance is ±50 Hz |
| Modulation and speed levels | Implemented | Nine levels, 1–9, using BPSK, QPSK, 8PSK, 16-QAM and 64-QAM on OFDM carriers; HF supports 1–8, and level 9 is VHF-only |
| Payload FEC | Implemented | One NR BG2 LDPC mother code with per-level shortening and rate matching |
| Frame protection | Implemented | Golay-protected header with CRC-16; CRC-32 for each payload codeword |
| ARQ and protocol stack | Implemented, opt-in ARQ | Selective-repeat ARQ, session management and optional Huffman + LZ4 compression; daemon ARQ defaults off |
| CLI | Implemented | Loopback and WAV transmit/receive; live audio requires `cpal-backend` |
| Daemon event loop | Implemented | Processes audio and host events; live audio requires `cpal-backend` |
| VARA-style TCP interface | Implemented, opt-in | Control/data ports default to 8300/8301; server disabled by default; not RF-compatible with VARA |
| C FFI | Implemented | Shared/static library with one-shot encode/decode and streaming decode |
| Operating profiles | Implemented | 48 kHz OFDM profiles with runtime speed-level selection within the configured band |
| Link adaptation | Implemented, limited calibration | Measurement-driven speed selection and ACK-driven RateLoop with ARQ enabled; no model inference; performance targets remain unmet, see [BENCHMARKS.md](BENCHMARKS.md) |
| Serial/GPIO PTT | Implemented, feature-gated | Daemon serial DTR/RTS uses `serial-ptt`; Linux sysfs GPIO uses `gpio-ptt` |
| IR-HARQ | Unwired codec primitive | Redundancy-version and combining support exists in the codec; engine/daemon retransmissions do not wire it into an operational IR-HARQ feature |
| VARA RF/waveform compatibility | Not supported | The TCP control interface is VARA-style; Coppa does not interoperate with VARA over RF |

## Features

- OFDM encode/decode with pilot-based channel estimation, channel equalization and streaming receive.
- Nine speed levels using OFDM carrier constellations from BPSK through 64-QAM; levels 1–8 on HF and 1–9 on VHF.
- NR BG2 LDPC payload coding with shortening, rate matching and interleaving; Golay and CRC-16 header protection, plus per-codeword CRC-32 payload checks.
- Selective-repeat ARQ and session management, with optional Huffman + LZ4 compression. Daemon ARQ is disabled by default.
- WAV file I/O and live CPAL audio with `cpal-backend`; daemon serial and Linux GPIO PTT with their respective feature flags.
- VARA-style TCP command/data interface for host applications, disabled by default and not RF-compatible with VARA.
- C FFI shared/static library with one-shot encode/decode and streaming decode.
- Deterministic link adaptation and spectrum sensing. Short-CP recommendation and negotiation are opt-in; IR-HARQ remains an unwired codec primitive.

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

# Decode a WAV file (prints the message text)
coppa rx -i cq.wav

# Show the decoded payload as hex bytes instead (--raw: hex only, for scripts)
coppa rx -i cq.wav --hex

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
| `coppa-protocol` | OFDM transceiver, NR BG2 LDPC payload coding, framing, ARQ, compression and session management |
| `coppa-channel` | Channel models for testing: AWGN, sinusoidal fading, frequency offset |
| `coppa-audio` | Audio backends: CPAL real-time I/O, WAV file read/write |
| `coppa-radio` | Radio control: rigctld CAT, serial PTT (DTR/RTS, `serial-ptt` feature), GPIO PTT (Linux sysfs, `gpio-ptt` feature) |
| `coppa-ml` | Measurement-driven speed selection, RateLoop, CpGate, BusyGate and spectrum sensing |
| `coppa-engine` | CoppaTransceiver wrapper with compression, squelch and streaming receive |
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
