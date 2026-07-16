---
id: adr-006-phase2-estimation
title: What did Phase 2's channel-estimation work fix, break, and leave open?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/006-phase2-parametric-estimation-nr-bg2.md
  - crates/coppa-protocol/src/modem/**
verified:
  commit: 59b0b63
  date: 2026-07-14
links:
  - overview
  - adr-004-strongest-path-timing
  - watterson-level-4-gap
  - adr-005-nr-bg2-ldpc
---
Phase 2 replaced the linear-interpolation channel estimator with a delay-domain
ridge-LS estimator (`DelayDomainEstimator`) plus a Kalman/RTS lag smoother
(`KalmanLagSmoother`), added a soft-decoded Golay+CRC-list header, known-pad
LLR pinning, and one-round turbo re-estimation. The wins are real, but the
estimator itself carries a **known, unresolved regression, shipped anyway** —
the most important open item in the receiver.

## What to know before touching this area

- **The regression:** Watterson-Moderate/level-2's FER≤10% threshold moved
  18 dB → 30 dB (it was supposed to improve). Root cause: the estimator's
  frame-global coarse-delay reference doesn't track real intra-frame drift; a
  per-window adaptive fix was built and reverted (it corrupted the LDPC-facing
  noise variance). The Kalman tracker didn't close it — believed to be a
  model-class mismatch (drift looks like accumulating phase/delay reference
  error, not the AR(1) amplitude fading the tracker models). Not a tuning
  problem; sweeping the forgetting coefficient across ~2 orders of magnitude
  was FER-flat.
- **The CFO trap and its proven fix pattern:** CFO-induced sync-timing jitter
  desyncs the estimator's `calibrated_bias` reference and once collapsed
  level 4 under 40 Hz CFO to an SNR-unresponsive FER floor. Fixed by
  `CoppaModem::bounded_coarse_delay` (`COARSE_DELAY_JITTER_BOUND = 0.15`) —
  and the "obvious" wider bound (0.5) was directly measured to be a disguised
  Watterson regression. This `SyncDetector`-timing + `calibrated_bias` code
  area has now bitten three times (ADR-003/004/006); any change here must be
  verified against BOTH CFO and Watterson-fading cases, together.
- **Turbo re-estimation's benefit is concentrated at BPSK** (rescues 21–50% of
  first-pass failures at level 2; only 1–2% at levels 5/6 under Poor).
- **FEC-layer gains can be invisible end-to-end**: at some operating points
  OFDM sync, not LDPC, is the binding constraint — verify FEC/LLR improvements
  with an isolated (OFDM-bypassing) bench, not just the full pipeline.

Full decision record: `docs/adr/006-phase2-parametric-estimation-nr-bg2.md`;
investigation detail in `.superpowers/sdd/p2-task-1-report.md`,
`p2-task-7-report.md`, and `p2-cfo-level4-{investigation,fix}-report.md`.
