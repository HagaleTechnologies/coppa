---
id: ldpc-non-convergence
title: What will bite you about LDPC at speed levels 9 and 10?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa/tests/phase_c_loopback.rs
  - crates/coppa-protocol/src/fec/ldpc/**
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - coppa-protocol
  - adr-002-fec-strategy
---
Speed levels 9 and 10 (64-QAM with 7/8 and 3/4 LDPC) fail to decode reliably
even at high SNR in `crates/coppa/tests/phase_c_loopback.rs`. The decoder exits
without convergence, producing bit errors that are not a PAPR-clipping problem
(despite levels 9/10 having the highest PAPR targets, 14 dB at 64-QAM). The
root cause is a decoder/code-rate issue, not a signal conditioning one.

## Symptom

`phase_c_loopback` tests at levels 9 and 10 report decode failures or high BER
at SNRs where lower speed levels decode cleanly. The LDPC belief-propagation
decoder hits its iteration limit (50) without reaching a valid codeword.

## Cause and workaround

64-QAM's dense decision regions mean any residual channel-estimation error or
timing imperfection produces enough soft-decision noise to prevent LDPC
convergence. This is documented as a known limitation in `CLAUDE.md` ("levels
9/10 (64-QAM) hitting LDPC non-convergence at high SNR ... a decoder/code-rate
issue, not a PAPR-clipping one"). The 64-QAM constellation mappers are
implemented and verified, but the full integration is not robust at these levels.

Workaround: use speed levels 1–8 for reliable operation. Do not add or depend on
levels 9/10 paths in production code until this is investigated further.
