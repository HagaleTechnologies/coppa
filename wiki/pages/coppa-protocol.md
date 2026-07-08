---
id: coppa-protocol
title: How does coppa-protocol handle framing, FEC, and ARQ?
kind: subsystem
status: current
maintainer: agent
sources:
  - crates/coppa-protocol/**
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - overview
  - adr-002-fec-strategy
  - ldpc-non-convergence
---
`coppa-protocol` contains the full protocol stack above the physical layer:
framing (V1/V2 PHY frames with preamble, sync, CRC-16), two FEC families
(convolutional and QC-LDPC), ARQ with selective repeat, Huffman + LZ4
compression, and session state management. This is also where
`CoppaTransceiver` lives — the struct that composes modem + LDPC +
constellation mappers + block interleaver into the encode/decode pipeline.

## How it works

- **Framing:** V1 and V2 PHY frames include preamble, sync word, length, payload,
  and CRC-16. The framing layer handles 180-degree phase ambiguity.
- **FEC:** Two implementations behind the `FecCodec` trait. Rate-1/2 K=7
  convolutional code (soft Viterbi) is used for BPSK; QC-LDPC at 6 rates
  (1/4 to 7/8) is used for OFDM modes. LDPC uses 1,944 coded bits at all
  rates. See [[adr-002-fec-strategy]] for the trade-off rationale.
- **ARQ:** Selective repeat with configurable window; negotiation not yet implemented.
- **Compression:** Fixed Huffman table for ham radio text + LZ4 for bulk data.
- **Session:** Connection state machine, callsign management.

## Why it is shaped this way

`CoppaTransceiver` was extracted here (rather than in `coppa-engine`) to keep
the protocol-layer tests self-contained without needing the engine orchestration.
The speed-level abstraction (9 levels replacing old mcs_index/fec_rate/modulation
config) lives here and in `coppa-engine`; see [[coppa-engine]].

## Known gotcha

LDPC levels 9/10 (64-QAM) show non-convergence at high SNR in the integration
tests. See [[ldpc-non-convergence]].
