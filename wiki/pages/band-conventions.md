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
- Coppa parses band labels in one place: the HF/VHF wideband regulatory
  gate (COP-40, `coppa_radio::band`). The coppa-dsp crate still operates on
  raw sample streams with no band awareness.
- The waveform passband (300–2700 Hz audio) is independent of RF frequency.
  Coppa modulates audio for injection into any SSB rig audio chain; the rig
  is tuned by the host via `coppa-radio` (rigctld CAT). The RF band conventions
  in dispensa ADR-0006 apply to the rigctld frequency argument, not to
  coppa's audio pipeline.

## The wideband regulatory gate (COP-40)

47 CFR 97.307(f)(3) caps US amateur data emissions below 29.7 MHz at 2.8 kHz
occupied bandwidth. `vhf_wide` occupies 5600 Hz, so `coppad`
(`EventLoop::transmit_samples`, plus a startup check) and `coppa tx`/`coppa tune`
refuse a wideband transmission unless frequency evidence places the station at
or above 29.7 MHz. Evidence, in precedence order: a live rigctld `f` answer,
then `[radio] frequency_hz` (`--frequency-hz`), then `[radio] band` (`--band`).
Narrowband HF profiles need no source. The band table in
`crates/coppa-radio/src/band.rs` is a minimal US subset (160m-23cm); dispensa
ADR-0006 remains the tenant-wide convention. See `coppad.toml.example` and the
COP-40 note in `CLAUDE.md`'s Known Limitations for what stays ungated.
