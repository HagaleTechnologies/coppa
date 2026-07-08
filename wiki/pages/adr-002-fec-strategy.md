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
  commit: c1d2676
  date: 2026-07-07
links:
  - overview
  - coppa-protocol
  - ldpc-non-convergence
---
Two FEC families cover the range of channel conditions: rate-1/2 K=7
convolutional code (low complexity, default for BPSK) and QC-LDPC at 6 rates
from 1/4 to 7/8 (higher complexity, intended for OFDM modes). Both expose
`encode()`/`decode()` behind the `FecCodec` trait so the engine can swap
without structural changes. Turbo codes are explicitly deferred.

## Digest

Convolutional FEC won for BPSK because it is well-understood and low-complexity.
LDPC won for OFDM because configurable code rate allows adaptation across a wide
SNR range. LDPC always uses 1,944 coded bits regardless of rate, which simplifies
frame sizing. The `FecCodec` abstraction is not yet fully wired through the
flagship modems — LDPC is implemented and tested in isolation but not in the
live engine data path.

Important caveat: no operating SNR curves are published; LDPC at levels 9/10
shows non-convergence issues (see [[ldpc-non-convergence]]).

Full rationale: `docs/adr/002-fec-strategy.md`
