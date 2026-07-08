---
id: waveform-wire-break
title: What will bite you about waveform compatibility with pre-Phase-1 code?
kind: gotcha
status: current
maintainer: agent
sources:
  - docs/adr/003-phase1-waveform-break.md
  - crates/coppa-codec/src/ofdm/**
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - adr-003-waveform-break
  - overview
---
The Phase 1 waveform is a hard wire-format break from all earlier coppa revisions.
Old and new waveforms are not interoperable — a pre-Phase-1 transmitter cannot be
decoded by a post-Phase-1 receiver, and vice versa. There is no negotiation
mechanism and no backward compatibility layer. If you have pre-Phase-1 WAV
recordings or test vectors, they will not decode with current code.

## Symptom

Attempting to decode a pre-Phase-1 WAV file with current `coppa rx` produces
no output or random garbage. The sync detector will not lock because the preamble
format and carrier layout are completely different.

## Cause

Three orthogonal changes each independently break the wire format:
1. **Carrier bin offset** — active carriers now start at bin 6 (~300 Hz) instead
   of bin 1 (~50 Hz). Bin assignments are baked into frame structure.
2. **Newman-phase in-band preamble** — replaced the full-Nyquist PN-BPSK comb;
   different bit pattern and energy distribution.
3. **`SyncDetector`** — the streaming O(1) detector is not compatible with the
   old batch `detect_coppa_version` search path (deleted).

All three changes are present from the first commit after Phase 1. The code does
not contain the old detector or preamble format. Full design rationale:
`docs/adr/003-phase1-waveform-break.md`.
