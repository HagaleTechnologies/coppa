---
id: coppa-engine
title: How does coppa-engine orchestrate the modem pipeline?
kind: subsystem
status: current
maintainer: agent
sources:
  - crates/coppa-engine/**
  - crates/coppa-protocol/src/**
verified:
  commit: 56361d6
  date: 2026-10-28
links:
  - overview
  - coppa-protocol
  - phase4-field-readiness
---
`coppa-engine` is a thin ~210-line wrapper (`CoppaCore`) around `CoppaTransceiver`,
which lives in `coppa-protocol`. The engine's job is profile selection, configuration
management, and exposing the encode/decode API to the application layer (CLI, daemon,
FFI). The main encode/decode logic is in `coppa-protocol`, not here.

## How it works

- **CoppaTransceiver** (in `coppa-protocol`) composes CoppaModem + LDPC +
  constellation mappers + block interleaver. This is the actual pipeline.
- **CoppaCore** (here) wraps CoppaTransceiver with profile-based configuration.
  Batch APIs: `encode`/`decode` (text) and byte-level equivalents. Streaming:
  `push_samples` feeds a `StreamingReceiver` and yields `StreamFrame`s — this is
  the path the daemon, FFI, and `coppa rx` all share, and it centralizes
  squelch/decompression handling.
- **`StreamFrame` payloads are raw bytes** (`payload: Result<Vec<u8>>`), not
  UTF-8 strings. This was a Phase 4 fix: the old text-only streaming API
  silently dropped every binary/compressed MAC PDU (i.e. all real session/ARQ
  traffic) at UTF-8 validation. Do not reintroduce `String`-typed payloads on
  any decode path (see [[phase4-field-readiness]]).
- **Speed levels 1-9** replace the old mcs_index/fec_rate/modulation
  triple; constants live in `coppa_codec::ofdm::coppa_modem::SPEED_LEVELS`. `set_speed_level`
  switches level at runtime (used by the rate loop and FFI v2); the OFDM band
  is set at construction time and kept across every level change (COP-23:
  `EngineConfig::ofdm_band`, pinned by a named profile, so an HF-configured
  engine stays HF at levels 5-8). COP-24 caps HF at level 8, 64-QAM 2/3.
  Level 9, 64-QAM 5/6, is VHF-only. `available_speed_levels()` supplies the
  daemon rate loop with the band-specific ladder; both `set_speed_level` and
  `encode_bytes_at_level` reject level 9 on HF. Old wire levels 9 and 10 became
  8 and 9 in 0.2.0. See `docs/adr/009-cop24-speed-ladder-renumber.md`. Only `EngineConfig::default()`
  (`OfdmBand::LevelDerived`, no band preference) still crosses to `vhf_wide` at
  level 5; `CoppaCore::set_ofdm_band` rebinds the band in place, and
  `occupied_bandwidth_hz_at(level)` previews a level's width — `coppad` uses
  both to keep a level-derived engine HF below 29.7 MHz (COP-21).
  `CoppaCore::on_vhf_profile()` reports which band is actually selected.
- **Named profiles:** `HF_ROBUST` (level 1), `HF_STANDARD` (level 2),
  `EMERGENCY` (level 1) and `VHF_FAST` (level 8). A named profile is a preset of
  band, speed level and compression. `EMERGENCY` is `HF_ROBUST` on the air: the
  two build identical engines, and `coppa profiles` says so. Every HF profile
  runs the `hf_standard` OFDM profile (or its negotiated short-CP twin), and
  `VHF_FAST` runs `vhf_wide`; the `hf_robust`/`hf_wide`/`hf_narrow`
  `CoppaProfile` constructors (codec-level, in
  `crates/coppa-codec/src/ofdm/mod.rs`, with `hf_standard_short_cp()`) are not
  reachable from a named profile. Pilot
  density still matters for fading: `hf_robust` uses dense pilots, the others
  sparse (see [[adr-004-strongest-path-timing]]).
- **`coppa_engine::ladder`** assembles each speed level's modulation, code rate,
  max bytes, air time and bandwidth per band from the encoder's own functions;
  `coppa profiles` prints it (COP-55).

## Why it is shaped this way

The thin-wrapper pattern keeps the engine crate fast to compile and avoids
coupling the test infrastructure to the full engine stack. Applications that
need only encode/decode (FFI, CLI loopback) get a minimal API surface. The
daemon and FFI both route streaming decode through `CoppaCore::push_samples`
rather than duplicating squelch/decompression/payload contracts.
