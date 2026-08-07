---
id: ldpc-non-convergence
title: What will bite you about LDPC at speed levels 9 and 10?
kind: gotcha
status: current
maintainer: agent
sources:
  - tests/phase_c_loopback.rs
  - crates/coppa-protocol/src/fec/ldpc/**
  - docs/adr/005-nr-bg2-ldpc.md
  - BENCHMARKS.md
verified:
  commit: be2141b
  date: 2026-08-05
links:
  - coppa-protocol
  - adr-002-fec-strategy
  - adr-005-nr-bg2-ldpc
---
The original gotcha this page described — levels 9/10 (64-QAM) failing to
converge even at high SNR in loopback — is **FIXED** by Phase 2's NR BG2 mother
code plus level 10's rate change from 7/8 to 5/6 (see [[adr-005-nr-bg2-ldpc]]).
`tests/phase_c_loopback.rs`'s `test_snr_fer_monte_carlo` now shows FER=0.00/100
for every level 1–10 across its whole swept SNR range. What remains is a
narrower, real, still-open level-9 problem under fading.

## What is fixed

Clean-channel and AWGN decode at levels 9/10 converges cleanly. Do not
re-add workarounds (skips, `#[ignore]`s) for the old high-SNR non-convergence —
stale `#[ignore]`s for exactly this were already removed once during the
Phase 2 merge.

## What still bites

Level 9 (64-QAM 2/3) clears AWGN at 21 dB after the stale IR-HARQ benchmark
state was fixed. Under Watterson fading, COP-4's 300-trial profile/CP matrix
still finds no profile clearing FER≤10% through 36 dB. The dominant real-receiver
failure is LDPC non-convergence, but a perfect-CSI oracle shows substantial
headroom; this is an open FEC-coverage/diversity limitation, not a proven
physical ceiling and not simply a short-CP failure.

## Related tuning trap

LDPC decoder-parameter changes (e.g. the normalized-min-sum alpha) must be
validated across the whole speed ladder and payload-size extremes, not a single
level: an alpha picked from a level-2-only sweep once broke level 10 to 100%
frame loss on a clean channel. See [[adr-005-nr-bg2-ldpc]].
