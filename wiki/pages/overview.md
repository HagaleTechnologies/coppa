---
id: overview
title: Coppa — what is this and where do things live?
kind: overview
status: current
maintainer: agent
sources:
  - README.md
  - ARCHITECTURE.md
  - CLAUDE.md
verified:
  commit: c1d2676
  date: 2026-07-07
links:
---
Coppa is an open-source OFDM digital modem for amateur radio written in Rust,
published as a reference implementation of an HF modem's DSP/FEC/protocol stack.
It is NOT RF/waveform-compatible with VARA and does not interoperate with it; the
VARA-style TCP interface mimics VARA's TNC API surface only. The most important
thing to know before touching it: all 12 crates operate at a fixed 48 kHz sample
rate; the waveform occupies a 300–2700 Hz SSB passband, which is a **wire-format
break** from pre-Phase-1 code — old and new waveforms are not interoperable (see
[[waveform-wire-break]] and [[adr-003-waveform-break]]).

## Where things live

- `crates/coppa-dsp/` — pure DSP primitives (FFT, filters, AGC, resampling); see [[coppa-dsp]]
- `crates/coppa-codec/` — modulation/demodulation (BPSK wired end-to-end; QPSK/QAM/OFDM partial)
- `crates/coppa-protocol/` — framing, FEC (convolutional + LDPC), ARQ, compression; see [[coppa-protocol]]
- `crates/coppa-channel/` — channel models for testing (AWGN, Watterson HF fading, ssb_filter)
- `crates/coppa-audio/` — CPAL real-time audio (feature-gated) + WAV I/O; see [[cpal-feature-gate]]
- `crates/coppa-radio/` — rigctld CAT control; serial/GPIO PTT are stubs
- `crates/coppa-ml/` — channel prediction (EWMA only; no ML model loading)
- `crates/coppa-engine/` — CoppaTransceiver + CoppaCore orchestration; see [[coppa-engine]]
- `crates/coppa-host/` — VARA-style TCP control server (ports 8300/8301), WebSocket API
- `crates/coppa-ffi/` — C FFI bindings (cdylib + staticlib)
- `crates/coppa-cli/` — CLI binary (`coppa`)
- `crates/coppa-daemon/` — daemon binary (`coppad`)
- `docs/adr/` — four ADRs covering workspace layout, FEC strategy, Phase 1 waveform, sync timing
- `BENCHMARKS.md` — measured performance data; normative for Phase 1 acceptance results

## Start here

New to the codebase: read [[coppa-engine]] to understand the encode/decode pipeline, then
[[adr-003-waveform-break]] to understand why the waveform is shaped the way it is. If working
on HF fading performance, read [[adr-004-strongest-path-timing]] and [[watterson-level-4-gap]]
before touching `SyncDetector`.
