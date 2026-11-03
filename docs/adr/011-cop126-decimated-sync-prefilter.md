# ADR-011: A decimated sync pre-filter on the streaming receive path

## Status

Accepted, COP-126, 2026-11-03.

## Context

The daemon, `coppa rx` and the FFI all receive through
`CoppaCore::push_samples`, which feeds raw 48 kHz audio to
`StreamingReceiver`. The receiver passed that audio straight to its
`SyncDetector`. The detector's Schmidl-Cox plateau test therefore saw noise
across the whole 24 kHz band. The batch API (`CoppaTransceiver::receive`)
band-passes to 250-2850 Hz before it runs `SyncDetector::detect_all`, so it
sees about 9 dB less noise for the same signal.

ADR-003 accepted this gap on purpose. A continuous 601-tap RX band-pass ahead
of the detector cost about 13 times the bare detector and broke the 0.005x
CPU budget. It named "a cheaper/shorter filter, or downsampling before
detection" as future work.

The gap was visible in the golden vectors. `L1_awgn12` and `L2_awgn12` (12 dB
AWGN, `hf_standard`) decoded through the batch API and produced no frame
through `coppa rx`, because the raw plateau metric peaked at 0.48 and 0.44,
under the 0.5 threshold. On `hf_standard` with full-band white noise, the
streaming receiver decoded 0 of 20 frames at 3, 6 and 10 dB at every level
from 1 to 7, where the batch API decoded most of them.

## Decision

1. **Decimate the coarse stage only.** `SyncDetector::new_prefiltered` runs
   a 121-tap 250-2850 Hz band-pass (`coppa_dsp::fir::DecimatingFir`) that
   keeps every 4th sample, and runs the Schmidl-Cox plateau search and its
   CFO estimate at 12 kHz on that copy. The confirm cross-correlation and the
   strongest-path timing step (ADR-004) still run at 48 kHz on raw samples.
2. **Map the coarse peak back to full-rate coordinates.** A coarse plateau
   peak at coarse sample `k` maps to input sample
   `k * 4 - 60` (the filter's group delay). The full-rate refine searches
   ±cp around that point, so candidate timing is unchanged: every golden
   vector's candidate is sample-identical to the unfiltered detector's.
3. **Opt in by constructor.** `StreamingReceiver::new` uses
   `new_prefiltered`. `SyncDetector::new` and `detect_all`, which the batch
   path calls on already-filtered input, are unchanged. A VHF profile
   (`phy_mode == 1`, no RX filter) or a profile whose geometry does not
   divide by 4 gets exactly `new`.
4. **Fix the parameters inside the constructor.** 121 taps, 250-2850 Hz
   (the HF RX filter's passband) and a factor of 4 are constants in
   `sync_detector.rs`. No public config type exists for them.

## Consequences

- The sync threshold on full-band AWGN at levels 1 and 2 moved from about
  12.3 dB to about 2.3 dB at the 50% point, a gain of about 10 dB (BENCHMARKS.md,
  "COP-126"). `coppa-bench`'s
  `prefilter_moves_the_sync_threshold_down_by_at_least_6_db_at_levels_1_and_2`
  guards at least 6 dB on every push.
- All 20 golden vectors decode through `StreamingReceiver`, `coppa rx` and
  the daemon's `handle_audio_in`.
- The streaming sync search costs less than before, because the plateau
  search runs on a quarter of the samples (cost line in BENCHMARKS.md,
  "COP-126").
- A candidate surfaces about 5 ms later (the filter's 60-sample delay plus the
  12 kHz Hilbert's 256-input-sample delay). The confirm step already waits
  about 58 ms after the coarse peak.
- The band is fixed. `hf_narrow` (carriers 350-800 Hz) still has most of
  its in-filter noise and gains less (`docs/LIMITATIONS.md`).
- The batch path is unchanged.

## Alternatives

Numbers from the COP-126 plan-phase prototype (release build, 10 s of noise in
512-sample chunks):

| option | sync threshold gain | CPU of `SyncDetector::push` | timing on Watterson-Poor vectors | why rejected |
|---|---|---|---|---|
| Full-rate 121-tap pre-filter ahead of `sync.push` | 9.9 dB | 0.0215x (2.8x unfiltered) | identical | Nearly triples the cost of an always-on loop. |
| Full-rate 601-tap pre-filter | 10.1 dB | 0.081x (11x unfiltered) | identical | ADR-003 already rejected this cost. |
| Decimated detector with a 12 kHz refine | 9.9 dB | 0.0055x | `L1_poor25` lost | Timing quantized to 4 samples; a one-sample shift breaks the header decode. |
| Lower `PLATEAU_THRESHOLD`, no filter | unmeasured | 1x | n/a | No false-alarm data, and it would also change the batch path. |
| 61-tap, decimate by 6 | 9.5 dB | 0.0024x | identical | 0.34 dB short of the chosen design; a later CPU trade if needed. |
| **Chosen: 121-tap, decimate by 4, full-rate refine** | **9.9 dB** | **0.0055x** | **identical** | |
