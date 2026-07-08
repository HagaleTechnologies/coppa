---
id: coppa-dsp-skimmer-interface
title: What contract does coppa-dsp expose to skimmer?
kind: interface
status: current
maintainer: agent
sources:
  - crates/coppa-dsp/src/**
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - coppa-dsp
  - overview
---
`coppa-dsp` is consumed by the skimmer repo as a pure-DSP library — it provides
FFT, FIR filtering, and AGC primitives that the skimmer uses in its decode core.
This is a cross-cutting dependency: coppa-dsp is public (MIT OR Apache-2.0),
skimmer vendors or depends on it directly. The interface is the crate's public API,
not a versioned schema, but changes to public types in `coppa-dsp` can silently
break skimmer's build.

## Pointers

- The skimmer spot-stream output (the JSON Lines/WebSocket schema that skimmer
  emits to cqdx) is a separate contract: dispensa Q-0028
  (`questions/0028-skimmer-spot-stream-contract.md`). That question is open;
  the agreed schema has not yet been written to `contracts/spots/`.
- The coppa-dsp crate itself has no versioned schema in dispensa; its contract
  is the Rust public API at each coppa release. Interface changes to `coppa-dsp`
  public items should be treated as potentially breaking for skimmer.
- Band and frequency conventions used when interpreting coppa-dsp output
  (e.g., mapping channelizer frequency bins to band labels) should follow
  dispensa ADR-0006 (`adr/0006-band-frequency-conventions.md`) and the canonical
  band table at `contracts/bands/bands.v1.json`.
