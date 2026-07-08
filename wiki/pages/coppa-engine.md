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
  commit: c1d2676
  date: 2026-07-07
links:
  - overview
  - coppa-protocol
---
`coppa-engine` is a thin ~210-line wrapper (`CoppaCore`) around `CoppaTransceiver`,
which lives in `coppa-protocol`. The engine's job is profile selection, configuration
management, and exposing the encode/decode API to the application layer (CLI, daemon,
FFI). The main encode/decode logic is in `coppa-protocol`, not here.

## How it works

- **CoppaTransceiver** (in `coppa-protocol`) composes CoppaModem + LDPC +
  constellation mappers + block interleaver. This is the actual pipeline.
- **CoppaCore** (here) wraps CoppaTransceiver and exposes `encode(text)` /
  `decode(samples)` with profile-based configuration.
- **9 speed levels** replace the old mcs_index/fec_rate/modulation triple.
  All profiles are unified at 48 kHz sample rate. Constants live in
  `SPEED_LEVELS` in this crate.
- **Operating profiles:** HF_ROBUST, HF_STANDARD, HF_WIDE, HF_NARROW, VHF_FAST, etc.
  Profile selection affects carrier count, CP length, pilot density, and which
  FEC rate is used. The `hf_robust` profile uses 12 pilots; `hf_standard`,
  `hf_wide`, and `hf_narrow` use only 2-4 (sparse) — this sparse/dense distinction
  is critical for fading performance (see [[adr-004-strongest-path-timing]]).

## Why it is shaped this way

The thin-wrapper pattern keeps the engine crate fast to compile and avoids
coupling the test infrastructure to the full engine stack. Applications that
need only encode/decode (FFI, CLI loopback) get a minimal API surface.

## Current limitations

- `encode/decode` accept/return UTF-8 text only; binary payloads need
  `encode_bytes/decode_bytes`, not yet implemented.
- Runtime modulation switching is not implemented; the profile is set at
  construction time.
