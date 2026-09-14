# coppa — codebase map

Open-source OFDM digital modem for amateur radio (Rust), published as a reference
implementation of an HF modem's DSP/FEC/protocol stack: full DSP chain, protocol
stack with ARQ, an AFSK 1200/AX.25 TNC, CLI tools, a daemon, C FFI bindings, and a
VARA-style TCP control interface (modeled on VARA's TCP TNC API — the modem is
**not** RF/waveform-compatible with VARA). One Cargo workspace, 14 members, all
profiles unified at a 48 kHz sample rate across 9 speed levels. Tickets are
`COP-<n>`.

**Read `CLAUDE.md` first.** Its "Known Limitations" section is the authoritative,
frequently-amended record of what is measured, what is unproven, and what is
deliberately left unfixed — it is not summarized here, on purpose. `wiki/INDEX.md`
maps the wiki: an overview page, per-crate deep-dives, 8 numbered ADR pages, and
several "what will bite you" gotcha pages. The wiki is descriptive and always
loses conflicts with code and `docs/`.

## crates/

- **coppa-dsp** — pure DSP: FFT, filters, AGC, resampling. No audio I/O, no async.
  Also consumed by the sibling `skimmer` repo as a library.
- **coppa-codec** — modulation: BPSK, QPSK, 8PSK, QAM, OFDM. Holds `CoppaModem`
  and `SyncDetector` (`src/ofdm/`).
- **coppa-protocol** — framing, FEC (convolutional + LDPC), ARQ, compression,
  sessions, CP negotiation. Houses **`CoppaTransceiver`**
  (`src/modem/transceiver.rs`), the main encode/decode pipeline, which composes
  CoppaModem + LDPC + constellation mappers + block interleaver; plus
  `StreamingReceiver` (`src/modem/streaming.rs`) and `cp_negotiator.rs`.
- **coppa-channel** — channel models for testing: AWGN, two-tap Watterson /
  ITU-R F.1487 fading, CFO, and an `ssb_filter` SSB-passband helper.
- **coppa-audio** — audio backends: CPAL (feature-gated `cpal-backend`), WAV file
  I/O, ring buffer, VOX, resampler.
- **coppa-radio** — radio control via rigctld CAT.
- **coppa-ml** — adaptive link control: capacity-based MCS selection
  (`recommend_speed_level`, `src/mcs.rs`), closed-loop rate control (`RateLoop`),
  spread-gated short CP (`CpGate`), spectrum sensing (`BusyGate`). Deterministic
  and measurement-driven — **not** ML/inference-based, despite the crate name.
- **coppa-engine** — **`CoppaCore`** (`src/engine.rs`), a thin ~210-line wrapper
  around `CoppaTransceiver`.
- **coppa-host** — VARA-style TCP control server, WebSocket JSON API.
- **coppa-ffi** — C FFI (cdylib + staticlib) with streaming decode. Uses
  pointer-to-pointer semantics in `coppa_engine_destroy` to prevent double-free;
  `build.rs` generates `coppa.h`.
- **coppa-cli** — the `coppa` CLI binary.
- **coppa-daemon** — the `coppad` daemon binary. `src/event_loop.rs` is the
  largest file in the repo and holds the real end-to-end audio dispatch path.
- **coppa-bench** — benchmark and diagnostic examples (`examples/*.rs`). Not
  shipped; CI compiles benches but never runs these.

## tools/

- **gen_nr_bg2** — a separate workspace member; code generator for the NR BG2
  LDPC tables consumed by `coppa-protocol`.

## Root-level

- `src/lib.rs` — a 9-line re-export facade for the `coppa` package.
- `tests/` — workspace-level integration tests: `phase_c_loopback.rs`,
  `integration_test.rs`, `afsk_loopback.rs`, `daemon_loopback.rs`,
  `short_cp_profile.rs`, `proptest_roundtrip.rs`, plus
  `serena_project_config.rs` (this config's own guard).
- `benches/throughput.rs`, `examples/`, `testdata/golden/` (frozen decode
  regression WAVs + `manifest.toml`), `fuzz/` (cargo-fuzz harness, excluded from
  the workspace), `wiki/`, `docs/` (`adr/`, `DECISIONS/`, `SPEC.md`,
  `OPERATING.md`).

## Conventions

- MSRV 1.85.0; the toolchain is pinned to 1.98.0 in `rust-toolchain.toml`, which
  overrides `rustup default` for any bare `cargo` invocation in this tree.
- `cargo test --workspace --lib` for a fast sanity check; `cargo test --workspace`
  (integration + proptest) before pushing. CI runs the full suite with
  `--features cpal-backend,websocket` on Linux, plus clippy `-D warnings`, fmt,
  MSRV, a 3-OS platform matrix, cargo-deny, and a RustSec audit.
- `libasound2-dev` (ALSA headers) is required on Linux for anything that builds
  `coppa-audio`'s `cpal` dependency — CI installs it before every cargo job.
- Run `/wiki-update` after substantive work; see `wiki/INDEX.md`.
- Serena here is **read_only**: navigation only, never edits.
