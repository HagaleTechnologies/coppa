---
id: band-conventions
title: What band and sample-rate conventions does coppa use?
kind: interface
status: current
maintainer: agent
sources:
  - CLAUDE.md
  - crates/coppa-engine/src/**
  - crates/coppa-codec/src/ofdm/**
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - overview
  - coppa-engine
---
Coppa operates at a fixed 48 kHz sample rate across all profiles and speed
levels. The waveform occupies a 300–2700 Hz SSB audio passband (carrier offset
of 6 FFT bins). All profile constants (carrier count, cyclic prefix length,
symbol duration, pilot layout) assume 48 kHz; resampling to other rates is the
caller's responsibility via `coppa-dsp`'s resampling utilities.

## Pointers

- Band labels and frequency ranges for HF operation: dispensa ADR-0006
  (`adr/0006-band-frequency-conventions.md`) defines the canonical US-interim
  band table at `contracts/bands/bands.v1.json`. The canonical label form is
  lowercase (e.g. `"20m"`); the table gives `rangeLowHz`/`rangeHighHz` and
  standard FT8 dial frequencies.
- Coppa does not itself parse band labels or filter by band. The coppa-dsp
  crate operates on raw sample streams; interpretation of which HF band is
  in use is the host application's responsibility.
- The waveform passband (300–2700 Hz audio) is independent of RF frequency.
  Coppa modulates audio for injection into any SSB rig audio chain; the rig
  is tuned by the host via `coppa-radio` (rigctld CAT). The RF band conventions
  in dispensa ADR-0006 apply to the rigctld frequency argument, not to
  coppa's audio pipeline.
