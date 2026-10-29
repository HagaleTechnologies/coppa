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
| ARQ and protocol stack | Implemented | Selective-repeat ARQ, session management and optional Huffman + LZ4 compression; connected-session data is always acknowledged and retransmitted (stop-and-wait); sessionless ARQ (`arq_enabled`) defaults off |
| CLI | Implemented | Loopback and WAV transmit/receive; live audio requires `cpal-backend` |
| Daemon event loop | Implemented | Processes audio and host events; live audio requires `cpal-backend` |
| ARDOP TNC host interface | Implemented, opt-in | Command/data ports default to 8515/8516; `ardop` feature, disabled by default; host-protocol interface only — both ends of a session run coppa — see [docs/ARDOP.md](docs/ARDOP.md) |
| VARA-style TCP interface | Implemented, opt-in | Control/data ports default to 8300/8301; server disabled by default; not RF-compatible with VARA; verified with Pat (Winlink) — see [docs/PAT.md](docs/PAT.md) |
| C FFI | Implemented | Shared/static library with one-shot encode/decode and streaming decode |
| Operating profiles | Implemented | 48 kHz OFDM profiles with runtime speed-level selection within the configured band |
| Link adaptation | Implemented, limited calibration | Measurement-driven speed selection and ACK-driven RateLoop with ARQ enabled; no model inference; performance targets remain unmet, see [BENCHMARKS.md](BENCHMARKS.md) |
| Serial/GPIO PTT | Implemented, feature-gated | Daemon serial DTR/RTS uses `serial-ptt`; Linux sysfs GPIO uses `gpio-ptt` |
| Service files | Provided, not hardware-verified | systemd unit and macOS launchd agent in `contrib/`; Raspberry Pi 4 install guide in [docs/tutorials/raspberry-pi-install.md](docs/tutorials/raspberry-pi-install.md) |
| KISS TNC (AFSK 1200) | Implemented, bring-up and testing only | Needs the kiss-tnc and cpal-backend features; not a Direwolf replacement; see [docs/TNC.md](docs/TNC.md) |
| IR-HARQ | Unwired codec primitive | Redundancy-version and combining support exists in the codec; engine/daemon retransmissions do not wire it into an operational IR-HARQ feature |
| VARA RF/waveform compatibility | Not supported | The TCP control interface is VARA-style; Coppa does not interoperate with VARA over RF |

## Features

- OFDM encode/decode with pilot-based channel estimation, channel equalization and streaming receive.
- Nine speed levels using OFDM carrier constellations from BPSK through 64-QAM; levels 1–8 on HF and 1–9 on VHF.
- NR BG2 LDPC payload coding with shortening, rate matching and interleaving; Golay and CRC-16 header protection, plus per-codeword CRC-32 payload checks.
- Selective-repeat ARQ and session management, with optional Huffman + LZ4 compression. Connected-session data is always acknowledged and retransmitted; sessionless ARQ is disabled by default.
- WAV file I/O and live CPAL audio with `cpal-backend`, including exact device IDs, selectable input/output channels, and hardware sample-rate conversion; daemon serial and Linux GPIO PTT with their respective feature flags.
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

# Encode at a chosen speed level (HF carries 1-8; see coppa profiles)
coppa tx "CQ CQ CQ DE VK2ABC K" --level 4 -o cq.wav

# Decode a WAV file (prints the message text)
coppa rx cq.wav

# Stream one JSON object per decoded frame (pipe into a JSON-lines consumer)
coppa rx cq.wav --json | jq -c .

# Show the decoded payload as hex bytes instead (--raw: hex only, for scripts)
coppa rx cq.wav --hex

# Receive live from the default input device for 60 seconds (requires cpal-backend)
coppa rx --duration 60

# List audio devices and copy an exact ID (requires cpal-backend feature)
coppa devices
coppa devices --json

# Check audio devices and rigctld without transmitting (pass/fail table)
coppa doctor

# Capture the right lane of a stereo input device
coppa rx --device-id "<id-from-devices>" --input-channel 1 --duration 60

# Show the speed levels and named profiles (add --json for machine-readable output)
coppa profiles
coppa profiles --level 4
coppa profiles --json
coppa profiles -p HF_STANDARD --json

# Suppress status output, or repeat -v for frame metrics on stderr
coppa -q rx cq.wav
coppa -vv rx cq.wav
```

`coppa` can keep callsign, profile, audio device and rig settings in the same `coppad.toml` as the daemon, with `COPPA_*` environment and flag overrides; see [CLI settings](docs/tutorials/getting-started.md#persisting-your-cli-settings).

`rx --json` writes newline-delimited objects with `payload_text` and exact
`payload_hex` bytes. `payload_text` is `null` for binary payloads. Diagnostics
go to stderr. `--json` conflicts with `--raw` and `--hex`; `-q`/`--quiet`
conflicts with `-v`/`--verbose`. `devices --json` and `profiles --json` write
arrays; `profiles -p NAME --json` and `profiles --level N --json` write one
object. `config` is a deprecated alias for `profiles` and accepts the same
flags; `listen` is a deprecated alias for `rx` and accepts the same flags.

## KISS TNC (AFSK 1200)

`coppa tnc` runs a 1200-baud AFSK (Bell 202) AX.25 TNC behind a KISS TCP port. Use it for
KISS bring-up and testing: checking that a KISS client, your audio routing and your PTT reach
coppa. It is not a Direwolf replacement. It has no AGWPE interface, no channel-busy check (it
ignores KISS TXDELAY, persistence and slot time), no digipeating or beaconing, and runs only at
1200 baud. That parity work is planned as H-122 in the
[2026-09-05 product review](docs/reviews/2026-09-05-product-ux-market-review.md).

[docs/TNC.md](docs/TNC.md) covers building it and a demo that runs two TNCs on one Linux machine.

## Workspace Crates

| Crate | Description |
|-------|-------------|
| `coppa-dsp` | Pure DSP primitives: FFT/IFFT, FIR design and filtering (windowed-sinc bandpass, Hilbert, root-raised-cosine), AGC, Costas carrier recovery, Gardner timing recovery |
| `coppa-codec` | Modulation/demodulation: BPSK, QPSK, 8PSK, QAM, OFDM, AFSK 1200 |
| `coppa-protocol` | OFDM transceiver, NR BG2 LDPC payload coding, framing, ARQ, compression and session management |
| `coppa-channel` | Channel models for testing: AWGN, Watterson HF fading, SSB passband filter, frequency, timing and sample-clock offsets, sinusoidal fading |
| `coppa-audio` | Audio backends: CPAL real-time I/O, WAV file read/write |
| `coppa-radio` | Radio control: rigctld CAT, serial PTT (DTR/RTS, `serial-ptt` feature), GPIO PTT (Linux sysfs, `gpio-ptt` feature) |
| `coppa-ml` | Measurement-driven speed selection, RateLoop, CpGate, BusyGate and spectrum sensing |
| `coppa-engine` | CoppaTransceiver wrapper with compression, squelch and streaming receive |
| `coppa-host` | Host interfaces: VARA-style TCP control server, WebSocket JSON API, KISS TNC TCP server (`kiss-tnc` feature) |
| `coppa-ffi` | C FFI bindings (cdylib + staticlib) with streaming decode API |
| `coppa-cli` | CLI binary (`coppa`): `tx`, `rx`, `loopback`, `tune`, `devices`, `profiles`, `config`, and `tnc` (`kiss-tnc` feature) |
| `coppa-daemon` | Long-running daemon (`coppad`) with event loop, audio, and host integration |
| `coppa-bench` | Measurement harness, not a product crate: BER/FER/goodput-vs-SNR scenarios, benchmark examples, and the lib tests that guard each speed level's AWGN waterfall on every push |

## Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full system design, TX/RX pipeline details, and layer responsibilities.

See [PLAN-hardening.md](PLAN-hardening.md) for known issues and the hardening roadmap.

See [docs/SPEC.md](docs/SPEC.md) for the normative waveform/FEC/protocol conformance specification — the reference for building an independent, interoperable implementation.

See [docs/OPERATING.md](docs/OPERATING.md) for field-operation guidance, including TX level calibration (`coppa tune`). To run coppad as a service (systemd or launchd), see [contrib/README.md](contrib/README.md); for a full Raspberry Pi 4 install, see [docs/tutorials/raspberry-pi-install.md](docs/tutorials/raspberry-pi-install.md).

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
