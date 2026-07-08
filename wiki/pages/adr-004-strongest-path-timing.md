---
id: adr-004-strongest-path-timing
title: Why does SyncDetector prefer the strongest multipath tap, not the first?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/004-strongest-path-timing.md
  - crates/coppa-codec/src/ofdm/sync_detector.rs
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - overview
  - adr-003-waveform-break
  - watterson-level-4-gap
---
Phase 1's `SyncDetector` originally anchored sync timing on the first-arriving
multipath tap, not the strongest. This caused a severe Watterson-fading regression
on sparse-pilot HF profiles (levels 1–4): ~30% FER floor under Watterson-Moderate
at SNRs that formerly decoded cleanly. ADR-004 bisected the fault to Task 5's
commit and switched to strongest-tap anchoring, with a fallback to first-path
only when the delay exceeds `cp_samples / 2` (150 samples on HF profiles).

## Digest

The mechanism: `hf_standard`'s 4-pilot linear interpolator cannot track the
composite two-tap channel response accurately when the FFT window is anchored on
the weaker, earlier tap instead of the energy-dominant tap. The fix prefers the
strongest correlation peak; the `cp/2` fallback preserves the original first-path
safety intent for genuine very-large-delay multipath. Levels 1–2 fully recovered;
level 3 very close; level 4 substantially improved but with a residual ~25% gap
not yet root-caused (see [[watterson-level-4-gap]]).

Full bisection data and before/after tables: `docs/adr/004-strongest-path-timing.md`
and `.superpowers/sdd/p1-fading-regression-fix-report.md`.
