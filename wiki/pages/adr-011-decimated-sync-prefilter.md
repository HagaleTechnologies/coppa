---
id: adr-011-decimated-sync-prefilter
title: Why does the streaming receiver's SyncDetector band-pass and decimate its input?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/011-cop126-decimated-sync-prefilter.md
  - crates/coppa-codec/src/ofdm/sync_detector.rs
verified:
  commit: 2526b50
  date: 2026-11-03
links:
  - adr-003-waveform-break
  - adr-004-strongest-path-timing
---
`StreamingReceiver` feeds raw 48 kHz audio to its `SyncDetector`, so the
plateau test used to see noise across the whole 24 kHz band, about 9 dB more
than the batch path sees after its RX band-pass. ADR-003 accepted that gap;
COP-126 closed it. `L1_awgn12` and `L2_awgn12` decoded through the batch API
but not through `coppa rx` or the daemon until then.

## Digest

- `SyncDetector::new_prefiltered` band-passes a copy of the input to
  250-2850 Hz with a 121-tap `coppa_dsp::fir::DecimatingFir` and keeps every
  4th sample. Only the Schmidl-Cox plateau search and its CFO estimate run
  at 12 kHz.
- The confirm and strongest-path timing steps (see
  [[adr-004-strongest-path-timing]]) still run at 48 kHz on raw samples, so
  candidate timing is unchanged. Decimating those too quantized timing to 4
  samples and lost `L1_poor25` in a prototype.
- `SyncDetector::new` and `detect_all` stay unfiltered. Use them on input
  that is already band-passed (the batch path); use `new_prefiltered` on raw
  receiver audio. VHF profiles get plain `new` either way.
- The band is fixed, so `hf_narrow` gains less. See `docs/LIMITATIONS.md`.

Measurements: BENCHMARKS.md, "COP-126". Rejected alternatives and their
numbers: `docs/adr/011-cop126-decimated-sync-prefilter.md`. Background on the
original trade-off: [[adr-003-waveform-break]].
