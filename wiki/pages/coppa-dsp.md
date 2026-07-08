---
id: coppa-dsp
title: How does coppa-dsp work and what does it expose?
kind: subsystem
status: current
maintainer: agent
sources:
  - crates/coppa-dsp/**
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - overview
  - coppa-dsp-skimmer-interface
---
`coppa-dsp` is the pure-DSP foundation of the workspace — no audio I/O, no async,
no feature flags. It depends only on `rustfft`, `num-complex`, and `smallvec`,
making it the crate most likely to be consumed by external tools. The skimmer
repo consumes it directly for FFT and filtering (see [[coppa-dsp-skimmer-interface]]).

## How it works

The crate provides:
- **FFT/IFFT** via `rustfft`-backed helpers with plan caching
- **FIR/IIR filters** including a 601-tap bandpass used in the HF waveform RX path
- **AGC** — block-adaptive, asymmetric attack/release
- **Resampling** — sample-rate conversion for non-48kHz audio sources
- **Signal analysis utilities** — RMS, peak, spectral helpers used by `coppa-ml`

All operations are on `f32` or `Complex<f32>`. There is no internal state shared
across threads; callers own and pass their own state structs.

## Why it is shaped this way

Isolation from hardware and async runtimes was a deliberate decision (see
[[adr-001-workspace-layout]]). By keeping `coppa-dsp` dependency-free beyond
`rustfft`, it compiles on embedded targets (no std required for the core
primitives) and can be vendored or consumed as a pure library without pulling
in CPAL, Tokio, or other heavy dependencies.
