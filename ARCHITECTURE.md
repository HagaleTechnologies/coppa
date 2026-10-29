# Coppa Architecture

## Executive Summary

Coppa is an open-source ham radio digital communications system written in Rust, published as a **reference implementation** of an OFDM HF modem (DSP + FEC + protocol). It prioritizes clarity, correctness, and maintainability. Coppa is not a finished on-air product and does not interoperate with existing proprietary modes such as VARA: its TCP control interface is modeled on VARA's TCP TNC API, but the waveform is not VARA-compatible. Its value is a clean, well-tested, readable Rust implementation of the underlying techniques (OFDM, LDPC, ARQ).

The normative wire format lives in [`docs/SPEC.md`](docs/SPEC.md). This document maps that format onto the crates and functions that implement it. When the two disagree, SPEC.md and the code win.

## Core Design Principles

1. **Modular Architecture**: Each protocol and modem type is a separate crate
2. **Clean Interfaces**: Well-defined trait boundaries between layers
3. **Incremental Development**: Start small, prove concepts, then expand
4. **Cross-Platform**: Works on Linux, macOS, Windows, and embedded systems
5. **Performance**: Optimized DSP pipeline for real-time operation

## System Architecture

### Crate dependency graph

Internal path dependencies, read from each crate's `[dependencies]` table (dev-dependencies are not shown):

```
coppa-dsp        (no internal dependencies)
coppa-ml         (no internal dependencies)
coppa-radio      (no internal dependencies)
coppa-codec      → dsp
coppa-audio      → dsp
coppa-channel    → dsp                                            test and bench use only
coppa-protocol   → codec, dsp, ml
coppa-engine     → dsp, codec, protocol
coppa-host       → engine
coppa-ffi        → engine
coppa-daemon     → engine, audio, radio, host, protocol, codec, dsp, ml
coppa-cli        → engine, audio, radio, codec, protocol, host, daemon
coppa-bench      → codec, protocol, channel, ml                   measurement harness, not a product crate
gen_nr_bg2       (tools/gen_nr_bg2, no internal dependencies)     code generator for the LDPC base graph
```

- `coppa-protocol` depends on `coppa-ml` because `CoppaTransceiver` calls `coppa_ml::recommend_speed_level` on every decoded frame. `coppa-engine` reaches `coppa-ml` only through `coppa-protocol`.
- `coppa-cli` depends on `coppa-daemon` for `coppa_daemon::tnc`, which backs the CLI's `tnc` subcommand.
- `coppa-protocol` has a dev-dependency on `coppa-bench`: its golden-vector test reuses the harness's profile selection.
- The root package `coppa` (`src/lib.rs`) only re-exports `coppa-codec`, `coppa-dsp`, `coppa-engine` and `coppa-protocol`. It owns the workspace integration tests and the criterion benchmark.

### Component Responsibilities

#### DSP Primitives (`coppa-dsp`)
- FFT/IFFT via rustfft (`FftProcessor`)
- FIR design and filtering: the windowed-sinc bandpass the OFDM TX/RX conditioning uses (`design_bandpass`), a Hilbert design, and a streaming FIR
- Older single-carrier primitives (AGC, a matched pulse-shaping filter, carrier and symbol-timing recovery) that only the legacy `BpskModem` uses; `benches/throughput.rs` also uses the AGC and the pulse-shaping filter directly. See [Legacy code not on the data path](#legacy-code-not-on-the-data-path)
- There is no sample-rate converter on the audio path, and nothing checks the rate. Every `CoppaProfile` runs at 48 kHz. The daemon hands its `[audio] sample_rate` (default 48000) to the sound card unchanged, and `coppa rx --input` does not compare a WAV file's rate with the engine's. `crates/coppa-audio/src/resampler.rs` is not compiled: `coppa-audio`'s `lib.rs` does not declare it, and the `rubato` crate it imports is in no `Cargo.toml`

#### Codec Layer (`coppa-codec`)
- **Constellation mappers** with hard and soft (LLR) demapping: `BpskMapper`, `QpskMapper`, `Psk8Mapper`, `Qam16Mapper`, `Qam64Mapper` (SPEC §6.1). These map bits to OFDM carrier values. None of them is a standalone modem
- **Coppa OFDM stack** (`ofdm/`): `CoppaProfile`, `CoppaModem` (frame modulation, demodulation and TX conditioning), the streaming `SyncDetector`, `CoppaPilotPattern`, `CoppaHeader` with Golay(24,12) + CRC-16 header FEC, the delay-domain channel estimator and Kalman tracker, `BlockInterleaver` and `CrossFrameInterleaver`
- **AFSK 1200** (Bell 202) modem for the AX.25 TNC mode
- **Traits**: `ConstellationMapper` is what the transceiver uses. `Modem`, `FecCodec` and `ChannelEstimator` are defined as extension points; the production path does not go through them

#### Protocol Layer (`coppa-protocol`)
- **Transceiver** (`modem/`): `CoppaTransceiver`, the encode/decode pipeline below; `StreamingReceiver`, the continuous RX front end; the speed-level table; frame airtime calculation
- **FEC** (`fec/`): one shared NR BG2 LDPC mother code (`NrLdpc`, Zc = 176) with circular-buffer rate matching and a layered normalized min-sum decoder (SPEC §5); the PRBS payload scrambler. The legacy codecs in `fec/` are listed under Legacy
- **ARQ**: selective-repeat `ArqTx`/`ArqRx` carried in `TransportPdu`s, plus `rv_for_attempt` for IR-HARQ redundancy-version cycling (not called by any production path; see the gaps list)
- **MAC, transport, application PDUs** and the **session** state machine (connect, data, disconnect, callsigns)
- **CP negotiation** (`cp_negotiator`): the propose/confirm/switched handshake that moves both stations between long and short cyclic prefix
- **Compression**: a fixed Huffman table tuned for ham radio text, followed by LZ4
- **AX.25**: a UI-frame codec, exercised by `tests/afsk_loopback.rs`

#### Channel Models (`coppa-channel`)
- AWGN at a configurable SNR, seeded, referenced either to the signal's own power or to a fixed clean reference
- Two-tap Watterson / ITU-R F.1487 HF channel (Rayleigh taps, Gaussian Doppler) with Good/Moderate/Poor presets
- `ssb_filter`, emulating a 300–2700 Hz SSB rig audio passband
- Carrier-frequency offset, timing offset and sample-clock offset impairments
- `fading`, a deterministic sinusoidal amplitude fade (periodic AM, not a fading model). Outside its own crate, its only callers are `CoppaCore` loopback tests in `tests/integration_test.rs`
- Used by tests and `coppa-bench` only; nothing shipped depends on it

#### Audio I/O (`coppa-audio`)
- **CPAL backend** (`cpal-backend`): real-time capture and playback through lock-free `rtrb` ring buffers
- **File backend** (`file-backend`): WAV read and write via hound
- A loopback backend, playback-progress bookkeeping and a VOX detector
- Device enumeration and selection

#### Radio Control (`coppa-radio`)
- **rigctld**: TCP client for hamlib CAT control (frequency, mode, PTT)
- **Serial PTT** (`serial-ptt`): DTR/RTS line control through the `serialport` crate
- **GPIO PTT** (`gpio-ptt`): `/sys/class/gpio` writes; real hardware access is Linux-only
- **VOX PTT** and a no-op PTT for soundcard-only operation

#### Channel Adaptation (`coppa-ml`)
- `recommend_speed_level`: the receiver's per-frame speed-level recommendation, computed from measured per-carrier noise variance through capacity-based MCS selection (`select_speed_level_2d` and related selectors), not a static SNR-threshold table
- `RateLoop`: sender-side closed-loop speed-level control, applying a receiver-computed recommendation fed back over the ACK, with raise-slow/drop-fast hysteresis
- `CpGate`: spread-gated short-CP profile recommendation from measured delay-spread history
- `BusyGate`: spectral-occupancy transition gate for daemon telemetry
- `SpectrumSensor`: FFT-based spectrum sensing and noise-floor estimation
- None of the above is ML/inference-based — all are deterministic functions of measurements the receiver already produces. The old EWMA channel-quality predictor and optional model-file registry were dead code (zero callers) and were removed in Phase 3

#### Engine (`coppa-engine`)
- `CoppaCore`: a thin wrapper around `CoppaTransceiver` that adds optional compression and squelch gating. Entry points: `encode_bytes`, `encode_bytes_at_level`, `decode_bytes`, `push_samples` (streaming), `set_speed_level`, `set_cp_profile`, `tune_tone`, plus the text wrappers `encode`/`decode`
- `EngineConfig` with `OfdmBand` (`LevelDerived`, `Hf`, `Vhf`), which pins the OFDM band across speed-level changes
- Named operating profiles: `HF_ROBUST` (level 1), `HF_STANDARD` (level 2, the daemon default), `EMERGENCY` (level 1, same on the air as `HF_ROBUST`), all HF; `VHF_FAST` (level 8, VHF)
- `coppa_engine::ladder` assembles each speed level's modulation, code rate, max bytes, air time and bandwidth from the code the encoder runs, for `coppa profiles`

#### Host Interfaces (`coppa-host`)
- **VARA-style TCP** (`vara-tcp`, default): command port 8300 and data port 8301, a VARA-inspired command set. The modem is not RF/waveform-compatible with VARA and does not interoperate with it over the air
- **WebSocket** (`websocket`): a JSON API server built on `tokio-tungstenite`, carrying callsign, connect/disconnect, send, received-data, status and spectrum messages; every handshake must present the `[host] websocket_token` and, from a browser, an allowlisted `Origin` (COP-53)
- **KISS over TCP** (`kiss-tnc`): the host side of the AFSK TNC mode

#### C FFI (`coppa-ffi`)
- One-shot: `coppa_engine_create`/`coppa_engine_new_with`/`coppa_engine_destroy`, `coppa_encode`, `coppa_encode_bytes`, `coppa_decode`, `coppa_engine_set_speed_level`
- Streaming: `coppa_engine_feed_samples` + `coppa_next_frame` (binary frames), and `coppa_start_stream`/`coppa_feed_samples`/`coppa_get_decoded`/`coppa_stop_stream` (text)
- `coppa_engine_destroy` takes a pointer to the handle pointer and nulls it, which prevents double-free
- cdylib + staticlib output; `build.rs` regenerates `coppa.h` with cbindgen

#### CLI (`coppa-cli`)
- Binary `coppa`. Subcommands: `tx`, `rx`, `loopback`, `tune`, `devices`, `config`, plus `tnc` when built with `kiss-tnc`. `listen` survives one release as a hidden, deprecated alias of `rx` (COP-56)
- `--config` (or `COPPA_CONFIG`) and other `COPPA_*` variables let `tx`, `tune`, `rx` and `loopback` share `coppad.toml` settings with the daemon
- WAV file I/O via `file-backend` (default); live audio capture and playback need `cpal-backend`

#### Daemon (`coppad`, crate `coppa-daemon`)
- Tokio event loop multiplexing audio, host clients (VARA-style TCP, WebSocket), radio control, ARQ timers, `RateLoop`, `CpGate`/CP negotiation and `BusyGate`
- Options: `--config <PATH>` (or a positional config path), `--check`, `--log-level`, `--print-default-config`, `--tnc`. Without a config path it uses the first file found of `./coppad.toml`, `$XDG_CONFIG_HOME/coppa/coppad.toml` (default `~/.config/coppa/coppad.toml`; `%APPDATA%\coppa\coppad.toml` on Windows) and `/etc/coppa/coppad.toml`, otherwise built-in defaults, and logs which one it used (COP-49)
- Hardware audio requires the `cpal-backend` feature; without it the daemon runs but moves no audio
- Ctrl-C graceful shutdown
- RX/TX audio level meter with clipping/underdrive warnings (logs + opt-in WebSocket `audio_level` stream)

#### AFSK 1200 / AX.25 TNC mode
- `coppad --tnc` or `coppa tnc`, both built with `kiss-tnc`, runs a separate mode: AFSK 1200 audio ↔ KISS TCP server (port 8001 by default) ↔ PTT. The KISS client supplies the AX.25 frames without an FCS, as KISS specifies. The TNC does not parse them, but it adds the FCS on transmit and checks and strips it on receive
- This mode does not use the OFDM pipeline below

## TX Pipeline

`CoppaCore::encode_bytes` → `CoppaTransceiver::transmit` → `CoppaModem::modulate_mapped`

```
payload bytes
  → optional Huffman + LZ4 compression (0xFE marker byte)          CoppaCore::encode_bytes
  → CoppaHeader: version, phy_mode, frame_type, bandwidth,
    redundancy version, speed_level, seq_num, payload_len, codewords  SPEC §4.2
  → split the payload across codewords                              SPEC §9.1
  → per codeword: CRC-32 trailer → zero-pad to k_used, then to the
    1760-bit info block → PRBS scramble → NR BG2 LDPC encode
    (Zc = 176) → rate-match to 1944 coded bits                      SPEC §5, §9.2
  → cross-codeword interleave (only when codewords > 1 and level ≥ 5) SPEC §7.2, §9.3
  → block interleave → constellation map per speed level
    (BPSK, QPSK, 8PSK, 16-QAM, 64-QAM)                              SPEC §6, §7.1, §9.4
  → OFDM at 48 kHz, 960-point FFT, 50 Hz carrier spacing:
    Newman-phase preamble (2 symbols) → PN probe symbol →
    Golay(24,12) + CRC-16 header on BPSK → payload symbols with pilots  SPEC §1–4
  → TX conditioning: section RMS leveling → raised-cosine symbol
    taper → PAPR clip at the level's target → 250–2850 Hz bandpass
    (HF profiles only) → peak normalize to 0.5                      SPEC §10
  → f32 audio samples
```

`CoppaCore::encode_bytes` builds every frame with one codeword, redundancy version 0 and `seq_num` 0. The multi-codeword and redundancy-version stages are exercised by `CoppaTransceiver`'s own callers and tests.

## RX Pipeline

`CoppaCore::push_samples` → `StreamingReceiver` → `CoppaTransceiver::receive_with_metrics` → `CoppaModem`

```
f32 audio samples
  → squelch gate (off unless squelch_threshold_db is set)           CoppaCore::push_samples
  → streaming preamble detection with two-stage CFO estimate
    (coarse half-symbol lag resolves the wrap of a fine
    full-symbol Moose lag; ±50 Hz)                                  SyncDetector, SPEC §3.1
  → header peek: CFO removal → header LLRs → soft Golay(24,12)
    decode with CRC-16-assisted list search                         SPEC §4.3
  → full frame: RX bandpass (HF only) → OFDM demodulation with
    probe-symbol delay calibration, per-symbol sample-clock-offset
    tracking, and Kalman-tracked pilot equalization                 SPEC §3.2–3.3 (receiver strategy)
  → soft demap to LLRs → de-interleave                              SPEC §6.1, §7
  → rate-dematch (single-codeword frames add into a per-seq_num
    LLR accumulator) → known-pad pinning → layered NR BG2 LDPC
    decode → one turbo re-estimation round if the decode does
    not converge                                                    SPEC §5.2–5.4, §5.6
  → descramble → per-codeword CRC-32 check → reassemble payload     SPEC §5.4–5.5, §9
  → decompress → StreamFrame (with speed-level recommendation and
    delay spread) to the daemon, FFI or CLI
```

`CoppaCore::decode_bytes` is the batch variant for a buffer that holds one frame. It calls `CoppaTransceiver::receive` and runs the same demodulation and decode stages.

### Profile choice

Profile choice happens when the engine is built or reconfigured, not on each frame. `CoppaCore::select_ofdm_profile` maps `OfdmBand` plus the negotiated `cp_mode` to one of three `CoppaProfile`s: `hf_standard` (long CP), `hf_standard_short_cp`, or `vhf_wide`. An `Hf` engine stays on HF at every level, a `Vhf` engine stays on `vhf_wide`, and a `LevelDerived` engine (`EngineConfig::default()`) uses HF for levels 1–4 and VHF from level 5. Both stations must already agree on the profile; it is not signaled in-band (SPEC §1.3). SPEC §1.1 lists four more profiles (`hf_robust`, `hf_narrow`, `hf_wide`, `vhf_narrow`); the engine never selects them, and they appear only in tests and benches.

The speed ladder has nine wire levels. HF tops out at level 8 (`HF_MAX_SPEED_LEVEL`); level 9 is VHF-only (ADR-009). SPEC §5.2 and §6 are authoritative for `k_used`, PAPR targets and bit mappings.

| Level | Constellation | Nominal code rate | Band |
|---|---|---|---|
| 1 | BPSK | 1/4 | HF + VHF |
| 2 | BPSK | 1/2 | HF + VHF |
| 3 | QPSK | 1/2 | HF + VHF |
| 4 | QPSK | 3/4 | HF + VHF |
| 5 | 8PSK | 2/3 | HF + VHF |
| 6 | 16-QAM | 1/2 | HF + VHF |
| 7 | 16-QAM | 3/4 | HF + VHF |
| 8 | 64-QAM | 2/3 | HF + VHF |
| 9 | 64-QAM | 5/6 | VHF only |

## Legacy code not on the data path

These components still compile and have tests, but no production TX or RX path calls them. Do not use them to understand how Coppa transmits.

- **`coppa_codec::BpskModem`**: a single-carrier BPSK modem with AGC, RRC pulse shaping, a Costas-loop carrier recovery, Gardner timing recovery and a 1 kHz carrier. Its only caller outside its own file is `benches/throughput.rs`. The `coppa-dsp` carrier recovery (`CostasLoop`), timing recovery (`GardnerTimingRecovery`), `RrcFilter` and `AdaptiveAgc` are used only by `BpskModem` and that benchmark. `examples/bpsk_loopback.rs` does not use it: it runs `CoppaCore` at level 1, which is BPSK on OFDM carriers.
- **`coppa_protocol::fec::convolutional`**: a rate-1/2 K=7 convolutional code with a soft Viterbi decoder. Callers: `benches/throughput.rs` and `fuzz/fuzz_targets/fuzz_viterbi.rs`.
- **`coppa_protocol::fec::ldpc::codes`**: the six-rate IEEE 802.11 QC-LDPC codes (Z = 81, N = 1944), with `LdpcCodec`, `LdpcEncoder` and `LdpcDecoder`. They are used only by `coppa-protocol` examples, `coppa-bench` examples and the harness's `transfer` module, and by unit tests. The module's `CodeRate` enum survives as a label returned by `speed_level_components`; `CoppaTransceiver` ignores it (SPEC §12).
- **`coppa_protocol::frame`**: the pre-OFDM frame (`[Preamble][Sync][Length][MAC PDU][CRC-16]`), re-exported as `coppa_protocol::Frame`. Callers: its own tests, `tests/proptest_roundtrip.rs` and `fuzz/fuzz_targets/fuzz_frame_parse.rs`.
- **The generic OFDM stack** in `coppa-codec/src/ofdm/mod.rs` (`OfdmProfile`, `OfdmModulator`, `OfdmDemodulator`, `equalizer::LinearInterpolationEstimator`): a single-symbol teaching example (SPEC §0.2). Outside `coppa-codec`, only `tests/integration_test.rs` uses it, and only `OfdmProfile`, `OfdmModulator` and `OfdmDemodulator`; `LinearInterpolationEstimator` is called only by `equalizer.rs`'s own tests. Its training symbols in `ofdm/frame.rs` (`SignalField`, `generate_sts`, `generate_lts`) are called only by that file's tests; the same file's `CoppaHeader` is on the production path.

## Current Status

- 12 product crates (~58,000 lines of Rust) plus the coppa-bench measurement harness (~12,600); ~74,000 lines across the Cargo workspace; 1,400+ tests (1,450 listed with CI's features). Counts: `find crates src tests benches examples tools -name '*.rs' | xargs cat | wc -l` and `cargo test --workspace --features cpal-backend,websocket -- --list | grep -c ': test$'`. The per-crate split comes from the same `find` run on each `crates/<name>` directory.
- The OFDM transceiver runs end to end through `CoppaCore`, `coppad`, the C FFI and the CLI, at nine speed levels (1–8 on HF, 1–9 on VHF), with runtime speed-level changes through `set_speed_level`
- Payload FEC is one shared NR BG2 LDPC mother code, rate-matched per speed level; the frame header is soft-decoded Golay(24,12) + CRC-16
- The streaming receiver corrects carrier-frequency offset up to ±50 Hz, tracks sample-clock offset within each frame, and runs one turbo re-estimation round when LDPC does not converge
- Selective-repeat ARQ for connected-session data (always, stop-and-wait; COP-38) and for no-session data (`arq_enabled`), closed-loop rate control from ACK-carried recommendations, and a CP negotiation handshake (off by default)
- Huffman + LZ4 compression; binary payloads through `encode_bytes`/`decode_bytes`
- Host interfaces: VARA-style TCP, a WebSocket JSON server, and KISS over TCP for the AFSK 1200 TNC mode
- Radio control: rigctld CAT, plus serial, GPIO and VOX PTT
- Tests: unit tests, workspace and per-crate integration tests, proptest round trips, 20 frozen golden WAV vectors (`testdata/golden/`, SPEC §14), cargo-fuzz targets, criterion benchmarks, and the `coppa-bench` harness. Simulated AWGN and Watterson results are in [BENCHMARKS.md](BENCHMARKS.md)
- CI: `ci.yml` runs check, lib tests with features, fmt and the platform matrix (Linux, macOS, Windows); `ci-full.yml` runs the full test suite, clippy, MSRV (1.85.0), bench and fuzz builds, cargo-deny, the security audit and the wiki lint

## Not implemented and known gaps

- Carrier-frequency-offset pull-in is ±50 Hz; sample-clock offset is tracked within each frame only.
- IR-HARQ is an unwired codec primitive: `CoppaTransceiver` supports redundancy versions and LLR combining, but `CoppaCore` always sends rv 0 and seq 0, and no engine or daemon path calls `rv_for_attempt`. The receive side still combines: a single-codeword frame's LLRs are added into the accumulator for its header `seq_num`, and only a CRC-verified decode, LRU overflow or a transceiver rebuild clears it. Because every `CoppaCore` frame carries seq 0, the LLRs of a frame whose LDPC decode or CRC check fails are added into the next frame's decode.
- Connected-session data runs stop-and-wait (one frame in flight, one ACK per frame) because the turnaround hold is receive-side only; a burst/block-ACK design is not implemented.
- No peer speed negotiation: the receiver's recommended level rides on each ACK, and the sender's `RateLoop` applies it on its own; `RateLoop` misses its own benchmark bar ([BENCHMARKS.md](BENCHMARKS.md)). Session connect exchanges `LinkCapabilities`, which include an `mcs_index`, but nothing applies the negotiated result.
- One ACK per decoded frame (block-ACK batching was considered and not chosen); multi-codeword frames retransmit whole, with no per-codeword ACK (ADR-007).
- Short-CP recommendation (`cp_gate_enabled`) and CP negotiation (`cp_negotiation_enabled`) default off.
- No over-the-air or two-radio measurements; every performance figure is a simulation.

## Dependencies

Workspace-pinned versions (`[workspace.dependencies]` in the root `Cargo.toml`):

| Purpose | Dependency | Pin | Used by |
|---|---|---|---|
| Errors | `anyhow` | 1.0 | audio, codec, protocol, radio, engine, host, cli, daemon |
| Numerics | `num-complex` | 0.4 | dsp, codec, protocol, channel, ml, bench |
| Numerics | `rustfft` | 6.2 | dsp, codec, ml |
| Checksums | `crc` | 3.2 | codec (header CRC-16), protocol (payload CRC-32), bench |
| Collections | `smallvec` | 1.13 | pinned, but no crate depends on it |
| Randomness | `rand` | 0.10 | channel, daemon, bench; dev-dependency elsewhere |
| Audio | `cpal` | 0.18 | audio (`cpal-backend`) |
| Audio | `hound` | 3.5 | audio (`file-backend`), bench |
| Audio | `rtrb` | 0.4 | audio |
| Radio | `serialport` | 4.9 | radio (`serial-ptt`) |
| Compression | `lz4_flex` | 0.14 | protocol |
| Async runtime | `tokio` | 1 (`rt-multi-thread`, `macros`, `net`, `sync`, `signal`, `io-util`, `time`) | radio, host, cli, daemon |
| Serialization | `serde` | 1 (`derive`) | host (`websocket`), daemon, bench |
| Serialization | `serde_json` | 1 | host and daemon (`websocket`) |
| Configuration | `toml` | 1.1 | daemon, bench |
| CLI parsing | `clap` | 4 (`derive`) | cli, daemon, bench |
| Logging | `tracing` | 0.1 | daemon |
| Logging | `tracing-subscriber` | 0.3 (`env-filter`) | daemon |
| Testing | `approx` | 0.5 | dev-dependency |
| Testing | `proptest` | 1.5 | dev-dependency (root, protocol) |
| Benchmarking | `criterion` | 0.8 (`html_reports`) | dev-dependency (root) |

Declared in a crate's own manifest rather than the workspace table:

| Crate | Dependency | Version | Role |
|---|---|---|---|
| `coppa-host` | `tokio-tungstenite` | 0.29 | optional, `websocket`: WebSocket server |
| `coppa-host` | `futures-util` | 0.3 | optional, `websocket` |
| `coppa-ffi` | `cbindgen` | 0.29 | build-dependency: generates `coppa.h` |

Feature flags (`[features]` tables):

| Crate | Default | Optional |
|---|---|---|
| `coppa-audio` | `cpal-backend`, `file-backend` | — |
| `coppa-cli` | `file-backend` | `cpal-backend`, `kiss-tnc` |
| `coppa-daemon` | none | `cpal-backend`, `websocket`, `kiss-tnc`, `serial-ptt`, `gpio-ptt` |
| `coppa-host` | `vara-tcp` | `websocket`, `kiss-tnc` |
| `coppa-radio` | none | `serial-ptt`, `gpio-ptt` |
| `coppa-ml` | none (`default = []`) | — |

The other crates declare no features. CI builds the full suite with `--features cpal-backend,websocket`. The `Cargo.toml` files win any conflict with these tables.

## Project Structure

```
coppa/
├── Cargo.toml                    # Workspace root (12 product crates, coppa-bench, tools/gen_nr_bg2; excludes fuzz/)
├── src/lib.rs                    # Root package: re-exports codec, dsp, engine, protocol
├── README.md                     # Project overview
├── ARCHITECTURE.md               # This document
├── BENCHMARKS.md                 # Simulated performance measurements
├── coppad.toml.example           # Annotated daemon configuration
├── crates/
│   ├── coppa-dsp/                # FFT, FIR, legacy single-carrier primitives
│   ├── coppa-codec/              # Constellation mappers, AFSK 1200, legacy BpskModem
│   │   └── src/ofdm/             # CoppaModem, SyncDetector, header FEC, channel estimation, interleavers
│   ├── coppa-protocol/           # CoppaTransceiver, ARQ, sessions, compression, CP negotiation, AX.25
│   │   ├── src/modem/            # CoppaTransceiver, StreamingReceiver, speed levels, airtime
│   │   └── src/fec/              # NR BG2 LDPC, scrambler, legacy codecs
│   ├── coppa-channel/            # AWGN, Watterson, SSB filter, CFO/SCO impairments (test and bench only)
│   ├── coppa-audio/              # CPAL and WAV backends, ring buffers, loopback, VOX
│   ├── coppa-radio/              # rigctld CAT, serial/GPIO/VOX PTT
│   ├── coppa-ml/                 # RateLoop, CpGate, BusyGate, MCS selection, spectrum sensing
│   ├── coppa-engine/             # CoppaCore, EngineConfig, named profiles
│   ├── coppa-host/               # VARA-style TCP, WebSocket, KISS
│   ├── coppa-ffi/                # C FFI bindings and generated coppa.h
│   ├── coppa-cli/                # CLI binary (coppa)
│   ├── coppa-daemon/             # Daemon binary (coppad), event loop, TNC mode
│   └── coppa-bench/              # Measurement harness and benchmark examples
├── tools/gen_nr_bg2/             # Transcribes 3GPP TS 38.212 BG2 and generates nr_bg2.rs
├── tests/                        # Workspace integration tests
├── benches/                      # Criterion benchmark (throughput.rs)
├── examples/                     # bpsk_loopback.rs, ofdm_roundtrip.rs
├── fuzz/                         # cargo-fuzz targets (outside the workspace)
├── testdata/golden/              # Frozen golden WAV vectors + manifest.toml (SPEC §14)
├── results/                      # Committed benchmark-run outputs
├── docs/                         # SPEC.md (normative wire format), OPERATING.md, adr/, tutorials/, DECISIONS/
└── wiki/                         # Descriptive knowledge wiki (INDEX.md + pages/)
```

## Goals

These are project goals, not validated results. The simulated AWGN and Watterson measurements in [BENCHMARKS.md](BENCHMARKS.md) come from software channel models. Coppa has no over-the-air or two-radio measurements, and makes no on-air throughput or bit-error-rate claims.

1. **Clarity**: readable, well-documented reference code for OFDM, LDPC, and ARQ
2. **Correctness**: high unit-test coverage of the DSP/FEC building blocks
3. **Portability**: builds on Linux, macOS, and Windows
4. **Efficiency (goal)**: real-time processing on modest hardware such as a Raspberry Pi 4
