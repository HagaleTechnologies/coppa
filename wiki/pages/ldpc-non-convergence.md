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
  commit: 59b0b63
  date: 2026-07-14
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

Level 9 (64-QAM 2/3) has an unusually high, steep, and strongly seed-dependent
AWGN SNR requirement (a real waterfall, not an SNR-independent floor), and
**never converges under any tested Watterson fading up to 54 dB** (Phase 3
Task 8 measurement, see `BENCHMARKS.md`'s "Phase 3 Task 8" section). This is
tracked as its own future investigation. If a bench or session run pins level 9
under fading and shows 100% loss, that is this known issue — not a regression.

## Related tuning trap

LDPC decoder-parameter changes (e.g. the normalized-min-sum alpha) must be
validated across the whole speed ladder and payload-size extremes, not a single
level: an alpha picked from a level-2-only sweep once broke level 10 to 100%
frame loss on a clean channel. See [[adr-005-nr-bg2-ldpc]].
