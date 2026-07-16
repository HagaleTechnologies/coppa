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
  commit: 59b0b63
  date: 2026-07-14
links:
---
Coppa is an open-source OFDM digital modem for amateur radio written in Rust,
published as a reference implementation of an HF modem's DSP/FEC/protocol stack.
It is NOT RF/waveform-compatible with VARA and does not interoperate with it; the
VARA-style TCP interface mimics VARA's TNC API surface only. The most important
thing to know before touching it: all 12 crates operate at a fixed 48 kHz sample
rate; the waveform occupies a 300–2700 Hz SSB passband, which is a **wire-format
break** from pre-Phase-1 code — and Phase 2's NR BG2 LDPC change and Phase 3's
multi-codeword frames are further wire breaks (see [[waveform-wire-break]],
[[adr-005-nr-bg2-ldpc]], [[adr-007-multi-codeword-frames]]). The normative
waveform definition is `docs/SPEC.md`; operator documentation is
`docs/OPERATING.md`.

## Where things live

- `crates/coppa-dsp/` — pure DSP primitives (FFT, filters, AGC, resampling); see [[coppa-dsp]]
- `crates/coppa-codec/` — modulation (BPSK/QPSK/8PSK/16-64QAM), OFDM, streaming `SyncDetector`
- `crates/coppa-protocol/` — framing, FEC (convolutional + NR BG2 LDPC), ARQ/IR-HARQ, compression; see [[coppa-protocol]]
- `crates/coppa-channel/` — channel models for testing (AWGN, Watterson HF fading, ssb_filter)
- `crates/coppa-audio/` — CPAL real-time audio (feature-gated) + WAV I/O; see [[cpal-feature-gate]]
- `crates/coppa-radio/` — rigctld CAT control + real serial (DTR/RTS) and Linux GPIO PTT (Phase 4)
- `crates/coppa-ml/` — deterministic link control: `RateLoop`, `CpGate`, `BusyGate` (not ML inference); see [[adr-008-phase3-system-layer]]
- `crates/coppa-engine/` — `CoppaCore` orchestration incl. streaming `push_samples`; see [[coppa-engine]]
- `crates/coppa-host/` — VARA-style TCP control server (ports 8300/8301), WebSocket API incl. spectrum stream
- `crates/coppa-ffi/` — C FFI bindings (cdylib + staticlib), v1 text API + v2 binary/config/event API
- `crates/coppa-cli/` — CLI binary (`coppa`), incl. live `rx` and two-tone `tune`
- `crates/coppa-daemon/` — daemon binary (`coppad`): PTT-gated TX chokepoint, busy gate, station ID/beacon; see [[phase4-field-readiness]]
- `docs/adr/` — eight ADRs (workspace, FEC, Phase 1 waveform, sync timing, NR BG2, Phase 2 estimation, multi-codeword, Phase 3 system layer)
- `docs/SPEC.md` — normative waveform conformance spec; `docs/OPERATING.md` — operator guide
- `BENCHMARKS.md` — measured performance data; normative for phase acceptance results

## Start here

New to the codebase: read [[coppa-engine]] to understand the encode/decode pipeline, then
[[adr-003-waveform-break]] to understand why the waveform is shaped the way it is. If working
on HF fading performance, read [[adr-004-strongest-path-timing]], [[adr-006-phase2-estimation]],
and [[watterson-level-4-gap]] before touching `SyncDetector` or the channel estimator. If
working on the daemon's TX path or real-radio behavior, read [[phase4-field-readiness]] first.
