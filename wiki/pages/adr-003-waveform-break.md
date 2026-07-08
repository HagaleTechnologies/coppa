---
id: adr-003-waveform-break
title: Why does Phase 1 break the waveform and what changed?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/003-phase1-waveform-break.md
  - BENCHMARKS.md
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - overview
  - waveform-wire-break
  - adr-004-strongest-path-timing
---
Phase 1 ("radio reality") moved the waveform from a full-Nyquist band (starting
at ~50 Hz) into the 300–2700 Hz SSB passband that a real HF transceiver actually
passes. This required changing carrier bin layout, the preamble, TX conditioning,
the sync detector, and CFO tolerance — all of which changed the wire format. There
is no backward compatibility; both ends of a link must run Phase-1-or-later code.

## Digest

Six locked decisions: (1) carrier offset of 6 bins (~300 Hz start), wire-format
break; (2) Newman-phase in-band preamble replacing full-Nyquist PN-BPSK comb, also
a break; (3) TX conditioning chain (per-section RMS leveling + PAPR clip + 601-tap
bandpass + 0.5 FS peak normalize) to fix a silent 9+ dB SNR penalty the preamble
change introduced; (4) RX bandpass + `ssb_filter` channel model added to
`coppa-channel`; (5) streaming O(1) `SyncDetector` replacing batch search; (6)
two-stage CFO acquisition for ±50 Hz tolerance. A Watterson-fading regression on
sparse-pilot HF profiles, discovered at the phase gate, was later fixed by
ADR-004. Levels 1–3 now match or exceed pre-Phase-1; level 4 is partially fixed
(see [[watterson-level-4-gap]]).

Full rationale and before/after benchmark tables: `docs/adr/003-phase1-waveform-break.md`
and `BENCHMARKS.md` ("2026-07 — Radio-reality Phase 1" section).
