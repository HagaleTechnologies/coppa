---
id: adr-008-phase3-system-layer
title: What did Phase 3's system layer ship, and which targets did it miss?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/008-phase3-system-layer.md
  - crates/coppa-ml/src/**
  - crates/coppa-bench/examples/**
verified:
  commit: 59b0b63
  date: 2026-07-14
links:
  - overview
  - coppa-protocol
  - adr-007-multi-codeword-frames
  - phase4-field-readiness
---
Phase 3 shipped the system layer: payload CRC-32, half-duplex ARQ discipline
(computed RTO floor, per-event backoff, u32 SACK), IR-HARQ, closed-loop rate
adaptation (`coppa_ml::RateLoop`), multi-codeword frames, SCO tracking + a
spread-gated short-CP profile, live daemon telemetry, and a MIL-STD/session/
golden-vector benchmark program. Two of its own acceptance bars are honestly
missed and documented — know them before "fixing" the wrong layer.

## The known shortfalls (measured, root-caused, open)

- **Rate loop misses its bar** (adaptive/best-fixed = 0.894 vs >1.0 required;
  adaptive/oracle = 0.751 vs ≥0.8): root-caused to the shared
  `channel_capacity` metric not being invariant to which speed level the
  measured frame used — `SPEED_LEVEL_MIN_CAPACITY` was only ever calibrated at
  a fixed level-2 probe, so climbing higher self-reinforces. This is a
  calibration/metric bug, NOT a `RateLoop` hysteresis bug; an 8-point
  `raise_dwell` sweep confirmed the shipped default is a genuine peak.
- **Benchmark program misses its targets**: `milstd` clears 0/27 operating
  points (the borrowed MIL-STD reference-SNR ladder doesn't transfer onto
  Coppa's real thresholds — a calibration mismatch, not a regression);
  `session` completes drop-free on only 3/5 Good, 0/5 Moderate/Poor (level 2's
  real nonzero Good-preset FER can exhaust the bounded ARQ retransmit budget
  in a low-SNR ramp trough — not an ARQ state-machine bug).
- **Block-ACK cadence (decision 4) was never implemented**: every decoded
  frame still triggers its own ACK — airtime-inefficient, not incorrect, and a
  documented gap against the plan's literal text.
- `CpGate`/`BusyGate` thresholds are synthetic-test-validated only, not swept
  against a real bench.

## Note on coppa-ml

Phase 3 deleted the old EWMA predictor/model registry (dead code) — `coppa-ml`
now contains only deterministic measurement-driven controllers (`RateLoop`,
`CpGate`, `BusyGate`). Nothing in it is ML/inference.

Full record (9 decisions): `docs/adr/008-phase3-system-layer.md`;
`BENCHMARKS.md`'s Phase 3 Task 4/Task 8 sections carry the measured data.
