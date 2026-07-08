---
id: watterson-level-4-gap
title: What will bite you about Watterson fading at speed level 4?
kind: gotcha
status: current
maintainer: agent
sources:
  - docs/adr/004-strongest-path-timing.md
  - crates/coppa-codec/src/ofdm/sync_detector.rs
  - .superpowers/sdd/p1-fading-regression-fix-report.md
  - BENCHMARKS.md
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - adr-004-strongest-path-timing
  - adr-003-waveform-break
---
Speed level 4 (QPSK 3/4 with sparse-pilot `hf_standard`/`hf_wide`/`hf_narrow`
profiles) is **partially fixed** after ADR-004 but retains a real, smaller
residual gap: ~72–76% of pre-Phase-1 peak goodput under Watterson fading,
improving from ~330–630 bps to ~555–1234 bps but not reaching full recovery.
Levels 1–3 are fully (or nearly fully) recovered.

## Symptom

Watterson-Moderate and Watterson-Poor benchmark sweeps for level 4 show lower
peak goodput than the pre-Phase-1 baseline even after the ADR-004 strongest-path
fix. AWGN performance at level 4 is unaffected and improved.

## Cause and workaround

The residual gap at level 4 is plausibly explained by QPSK's denser decision
regions being more sensitive to residual channel-estimate imperfection than
BPSK (levels 1–3), but this has not been further investigated. The problem is
not fully root-caused. Full measured data in `BENCHMARKS.md` ("2026-07 —
Hotfix: sparse-pilot header Watterson-fading regression" section) and in
`.superpowers/sdd/p1-fading-regression-fix-report.md`.

Workaround: if reliable Watterson fading performance is required, use levels
1–3 (BPSK). Do not assume level 4 is fully fixed; benchmark before relying on it.
