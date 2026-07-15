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
  commit: 59b0b63
  date: 2026-07-14
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
- **Speed levels 1–10 (8 reserved)** replace the old mcs_index/fec_rate/modulation
  triple; constants live in `SPEED_LEVELS` in this crate. `set_speed_level`
  switches level at runtime (used by the rate loop and FFI v2); the profile
  itself is still set at construction time.
- **Operating profiles:** HF_ROBUST, HF_STANDARD, HF_WIDE, HF_NARROW, VHF_FAST,
  plus a spread-gated short-CP HF profile (Phase 3). Profile selection affects
  carrier count, CP length, pilot density. `hf_robust` uses dense pilots;
  `hf_standard`/`hf_wide`/`hf_narrow` are sparse — this sparse/dense distinction
  is critical for fading performance (see [[adr-004-strongest-path-timing]]).

## Why it is shaped this way

The thin-wrapper pattern keeps the engine crate fast to compile and avoids
coupling the test infrastructure to the full engine stack. Applications that
need only encode/decode (FFI, CLI loopback) get a minimal API surface. The
daemon and FFI both route streaming decode through `CoppaCore::push_samples`
rather than duplicating squelch/decompression/payload contracts.
