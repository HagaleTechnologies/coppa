---
id: watterson-level-4-gap
title: What will bite you about Watterson fading at speed level 4?
kind: gotcha
status: current
maintainer: agent
sources:
  - docs/adr/004-strongest-path-timing.md
  - docs/adr/006-phase2-parametric-estimation-nr-bg2.md
  - crates/coppa-codec/src/ofdm/sync_detector.rs
  - docs/analysis/COP-134-fading-regressions.md
  - BENCHMARKS.md
verified:
  commit: d837c89d388eaf9c70752c72db8537d910094cc8
  date: 2026-11-04
links:
  - adr-004-strongest-path-timing
  - adr-003-waveform-break
  - adr-006-phase2-estimation
---
Speed level 4 (QPSK 3/4 with sparse-pilot `hf_standard`/`hf_wide`/`hf_narrow`
profiles) is **partially fixed** after ADR-004 but retains a real, smaller
residual gap: ~72–76% of pre-Phase-1 peak goodput under Watterson fading,
improving from ~330–630 bps to ~555–1234 bps but not reaching full recovery.
Levels 1–3 are fully (or nearly fully) recovered — and Phase 2 then changed
the fading landscape again (see below).

## Symptom

Watterson-Moderate and Watterson-Poor benchmark sweeps for level 4 show lower
peak goodput than the pre-Phase-1 baseline even after the ADR-004 strongest-path
fix. AWGN performance at level 4 is unaffected and improved.

## Cause and workaround

The residual gap at level 4 is plausibly explained by QPSK's denser decision
regions being more sensitive to residual channel-estimate imperfection than
BPSK (levels 1–2), but this has not been further investigated. The problem is
not fully root-caused. Full measured data is in [BENCHMARKS, the hotfix section](../../BENCHMARKS.md#2026-07--hotfix-sparse-pilot-header-watterson-fading-regression-adr-004)
and [ADR-004](../../docs/adr/004-strongest-path-timing.md).

Benchmark the intended profile and channel before relying on level 4. Levels 1–2
are BPSK; level 3 is QPSK 1/2 and is also in the later Moderate regression.
The historical recovery of levels 1–3 is not a current reliability guarantee.

## Phase 2 update

Phase 2's channel-estimator replacement made Watterson-Moderate performance a
mixed picture across the ladder: levels 1–2 improved (turbo re-estimation),
but levels 3–6 show a real regression at matched SNR, and level 4 separately
gained (then had fixed) a CFO-interaction failure. The estimator side of that
story — including the still-open 30 dB Moderate/level-2 threshold regression —
belongs to [[adr-006-phase2-estimation]]; read it together with this page
before touching anything in the sync/estimation area.

## COP-134 evidence review

[The COP-134 report](../../docs/analysis/COP-134-fading-regressions.md) keeps both
fading gaps open and supplies a controlled diagnosis plan with an unfiled follow-up
draft. QPSK sensitivity remains a hypothesis. The fixed CFO failure and later
level-2/session experiments do not establish a cause or remedy for these gaps.
