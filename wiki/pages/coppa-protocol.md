---
id: coppa-protocol
title: How does coppa-protocol handle framing, FEC, and ARQ?
kind: subsystem
status: current
maintainer: agent
sources:
  - crates/coppa-protocol/**
  - docs/SPEC.md
verified:
  commit: edb17bc
  date: 2026-11-04
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
  [[phase4-field-readiness]] for the storm this prevented. RV cycling exists
  in `arq.rs` but the daemon never uses it: every on-air frame, retransmissions
  included, is RV0 with PHY `seq_num` 0, and connected sessions run
  stop-and-wait (window 1). `docs/SPEC.md` §5.3 and §19 state the wire rules.
- **Rate adaptation:** the receiver computes a speed-level recommendation from
  per-carrier noise variances and feeds it back over the ACK; the sender's
  `RateLoop` (in `coppa-ml`) applies it (see [[adr-008-phase3-system-layer]]).
- **Compression:** fixed Huffman table for ham radio text + LZ4 for bulk data.
  Compressed MAC PDUs are binary — the decode path is bytes-first, not UTF-8.
- **Session:** connection state machine, callsign management. The normative
  wire format of the MAC, transport, ARQ and session layers (handshake,
  7-byte capabilities, keepalive, disconnect, optional CP-switch negotiation)
  is `docs/SPEC.md` §16–§20. §21 freezes hex vectors for them, and
  `crates/coppa-protocol/tests/protocol_vectors.rs` fails if a §21 row and
  the encoders disagree. Read the spec rather than this page for field layouts.

## Why it is shaped this way

`CoppaTransceiver` was extracted here (rather than in `coppa-engine`) to keep
the protocol-layer tests self-contained without needing the engine orchestration.
The speed-level abstraction lives here and in `coppa-engine`; see [[coppa-engine]].

## Known gotchas

- Level 9 (64-QAM 2/3) remains well above its FER target under tested Watterson
  fading, although a perfect-CSI COP-4 oracle proves implementation headroom.
  See [[ldpc-non-convergence]].
- Multi-codeword frames deliberately do NOT extend ACK addressing, turbo
  re-estimation, or persistent IR-HARQ combining to per-codeword granularity —
  enforced in code, not just documented. See [[adr-007-multi-codeword-frames]].
- Nothing on the wire separates a MAC frame from a bare transport PDU: the
  receiver tries MAC first (`docs/SPEC.md` §16.2–§16.3), so some opt-in
  sessionless ARQ PDUs parse as MAC frames and are dropped. Sessionless data
  PDUs also carry `ack=0`, which moves the receiver's own send window once its
  sequence numbers pass 128 (§19.5). Both are bullets in `docs/LIMITATIONS.md`;
  connected sessions are not affected.
