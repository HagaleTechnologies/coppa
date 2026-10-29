---
id: coppa-dsp
title: How does coppa-dsp work and what does it expose?
kind: subsystem
status: current
maintainer: agent
sources:
  - crates/coppa-dsp/**
verified:
  commit: 56361d6
  date: 2026-10-28
links:
  - overview
  - coppa-dsp-skimmer-interface
---
`coppa-dsp` is the pure-DSP foundation of the workspace — no audio I/O, no async,
no feature flags. It depends only on `rustfft` and `num-complex`,
making it the crate most likely to be consumed by external tools. The skimmer
repo consumes it directly for FFT and filtering (see [[coppa-dsp-skimmer-interface]]).

## How it works

The crate provides six modules:
- `fft::FftProcessor` — FFT/IFFT via `rustfft`-backed helpers with plan caching
- `fir` — windowed-sinc FIR design (`design_bandpass`, `design_hilbert`) and
  filtering (`Fir`, `StreamingFir`)
- `filter::RrcFilter` — root-raised-cosine FIR pulse shaping
- `agc::AdaptiveAgc` — block-adaptive, asymmetric attack/release
- `carrier_recovery::CostasLoop` — Costas carrier recovery
- `timing_recovery::GardnerTimingRecovery` — Gardner timing recovery

The 601-tap bandpasses on the waveform's TX/RX paths and the test SSB filter
are built from `design_bandpass` inside `coppa-codec`, `coppa-protocol` and
`coppa-channel`. The crate has no resampler and no IIR filter.

All operations are on `f32` or `Complex<f32>`. There is no internal state shared
across threads; callers own and pass their own state structs.

## Why it is shaped this way

Isolation from hardware and async runtimes was a deliberate decision (see
[[adr-001-workspace-layout]]). By keeping `coppa-dsp` dependency-free beyond
`rustfft`, it is a `std` crate (no `no_std` support) and can be vendored or
consumed as a pure library without pulling in CPAL, Tokio, or other heavy
dependencies.
