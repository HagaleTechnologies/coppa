---
id: adr-002-fec-strategy
title: Why does coppa use both convolutional and LDPC codes?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/002-fec-strategy.md
  - crates/coppa-protocol/src/fec/**
verified:
  commit: 59b0b63
  date: 2026-07-14
links:
  - overview
  - coppa-protocol
  - adr-005-nr-bg2-ldpc
  - ldpc-non-convergence
---
Two FEC families cover the range of channel conditions: rate-1/2 K=7
convolutional code (low complexity, soft Viterbi) and LDPC for the OFDM payload
path. Both expose `encode()`/`decode()` behind the `FecCodec` trait so the
engine can swap without structural changes. Turbo codes are explicitly deferred.

## Digest

Convolutional FEC won for the low-complexity side because it is well-understood
and cheap. LDPC won for OFDM because configurable code rate allows adaptation
across a wide SNR range.

**Superseded detail:** this ADR's original LDPC design (six separate 802.11
QC-LDPC base matrices at a fixed 1,944-bit codeword) was replaced in Phase 2 by
a single 5G-NR-style BG2 mother code (Zc=176) rate-matched per speed level via
a circular buffer — a wire-format break. The two-family strategy itself stands;
the LDPC family's internals are now governed by [[adr-005-nr-bg2-ldpc]].

The old caveat about levels 9/10 non-convergence at high SNR is fixed (by the
BG2 change plus level 10 moving 7/8 → 5/6); a narrower level-9-under-fading
issue remains open — see [[ldpc-non-convergence]].

Full rationale: `docs/adr/002-fec-strategy.md` (original decision) and
`docs/adr/005-nr-bg2-ldpc.md` (current LDPC layer).
