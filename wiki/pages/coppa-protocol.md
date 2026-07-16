---
id: coppa-protocol
title: How does coppa-protocol handle framing, FEC, and ARQ?
kind: subsystem
status: current
maintainer: agent
sources:
  - crates/coppa-protocol/**
verified:
  commit: 59b0b63
  date: 2026-07-14
links:
  - overview
  - adr-002-fec-strategy
  - adr-005-nr-bg2-ldpc
  - adr-007-multi-codeword-frames
  - adr-008-phase3-system-layer
  - ldpc-non-convergence
---
`coppa-protocol` contains the full protocol stack above the physical layer:
framing with a soft-decoded protected header (Golay ML + CRC-assisted list-2),
payload CRC-32, two FEC families (convolutional and NR BG2 LDPC), half-duplex
selective-repeat ARQ with IR-HARQ, Huffman + LZ4 compression, and session state
management. This is also where `CoppaTransceiver` lives — the struct that
composes modem + LDPC + constellation mappers + block interleaver into the
encode/decode pipeline.

## How it works

- **Framing:** frames carry a Golay-protected, soft-decoded header and a
  CRC-32-protected payload; multi-codeword frames (up to `MAX_CODEWORDS=8`)
  interleave across codewords within a frame (see [[adr-007-multi-codeword-frames]]).
- **FEC:** convolutional (soft Viterbi) for the AFSK/TNC side; the OFDM payload
  path uses a single 5G-NR-style BG2 LDPC mother code (Zc=176) rate-matched per
  speed level via a circular buffer — not per-rate codes (see
  [[adr-005-nr-bg2-ldpc]]). Known-pad LLR pinning and exact max-log LLR scaling
  are part of the decode path.
- **ARQ:** half-duplex selective repeat with a computed RTO floor, per-event
  backoff, u32 SACK, bounded retransmit budget, and IR-HARQ (RV-cycled
  retransmissions with additive LLR combining). Retransmit bookkeeping gotcha:
  every retransmission must call `ArqTx::mark_retransmitted` — see
  [[phase4-field-readiness]] for the storm this prevented.
- **Rate adaptation:** the receiver computes a speed-level recommendation from
  per-carrier noise variances and feeds it back over the ACK; the sender's
  `RateLoop` (in `coppa-ml`) applies it (see [[adr-008-phase3-system-layer]]).
- **Compression:** fixed Huffman table for ham radio text + LZ4 for bulk data.
  Compressed MAC PDUs are binary — the decode path is bytes-first, not UTF-8.
- **Session:** connection state machine, callsign management.

## Why it is shaped this way

`CoppaTransceiver` was extracted here (rather than in `coppa-engine`) to keep
the protocol-layer tests self-contained without needing the engine orchestration.
The speed-level abstraction lives here and in `coppa-engine`; see [[coppa-engine]].

## Known gotchas

- Level 9 (64-QAM 2/3) never converges under tested Watterson fading; the old
  high-SNR levels-9/10 non-convergence is fixed. See [[ldpc-non-convergence]].
- Multi-codeword frames deliberately do NOT extend ACK addressing, turbo
  re-estimation, or persistent IR-HARQ combining to per-codeword granularity —
  enforced in code, not just documented. See [[adr-007-multi-codeword-frames]].
