---
id: low-rate-floor
title: What has the low-rate floor experiment measured?
kind: gotcha
status: current
maintainer: agent
sources:
  - BENCHMARKS.md
  - docs/LIMITATIONS.md
  - crates/coppa-bench/src/floor_candidate.rs
  - crates/coppa-bench/src/floor_sweep.rs
verified:
<<<<<<< HEAD
  commit: 19948e9
  date: 2026-11-12
=======
  commit: 9295659
  date: 2026-11-11
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
links:
  - coppa-protocol
  - band-conventions
---
# What has the low-rate floor experiment measured?

COP-187's batch header repair reduces experimental candidate errors on paired
development seeds. It does not select a production waveform. The remaining
Poor failures still violate the separate acquisition bar, despite a favorable
FER crossing. See [measurements](../../BENCHMARKS.md#cop-187-experimental-header-repair)
<<<<<<< HEAD
and [unmet requirements](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-session-integration-open).
=======
and [unmet requirements](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-not-built-yet).
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409

Joint length/CRC decoding uses redundancy that independent hard nibble decisions
discard. A legal header hypothesis alone cannot justify delivery. Preserve the
version check, evidence threshold, payload CRC and padding checks, and continue
to count failed headers in SyncFailed.

<<<<<<< HEAD
The Capture adapter bounds history but requires an explicit batch decode.
COP-189's separate `CandidateStream` decodes overlapping windows once per second
and suppresses delivery of the same frame from successive windows. The
`floor_candidate --streaming` diagnostic uses it. A caller must supply trailing
audio through the next one-second decode interval. Work and retention bounds,
and the remaining production gap, are in
[LIMITATIONS](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-session-integration-open).
The experimental header value 2 does not allocate a production air version.

Run the long noise regression explicitly with
`cargo test --release -p coppa-bench --lib continuous_acquisition_300_seconds_noise -- --ignored`.
It covers seeded white noise, independently SSB-filtered one-second noise blocks
and a 1 kHz tone plus noise. It does not establish RF false-alarm probability
or a fading FER crossing.

`floor_mode sweep --rx batch|streaming|both` measures the production batch and
streaming receive paths. COP-191 adds `--waveform floor` using the production
`FloorFrame` and `FloorStream`, with one impaired capture per seed fed to both.
The noisy tail must cover the continuous receiver's next scheduled decode.
See [production sweep results](../../BENCHMARKS.md#cop-191-production-floor-sweeps)
for measured verdicts. Both receivers fail the sync-share bar on both channels,
with zero wrong payloads. `connect` and `ramp` still need COP-190's deferred
production session controller. A codec receive test does not prove session
establishment, floor ACKs or OFDM recovery.


CFO search in the experimental batch and continuous receiver uses thirteen
hypotheses from -150 to +150 Hz in 25 Hz steps. Its matched-filter recurrence
must use separate incoming and outgoing phases: half-bin templates repeat every
two symbols. Reusing the old one-symbol recurrence breaks the +/-75 Hz cases.
The bound is now sixteen sync candidates per hypothesis, or 208 per window.
See [the regression and remaining work](../../BENCHMARKS.md#cop-189-cfo-acquisition-repair).
This search does not supply production framing, negotiation or fading tracking.
=======
The Capture adapter bounds history but requires an explicit batch decode. Its
chunk-partition test does not prove continuous acquisition. The experimental
header value 2 does not allocate a production air version.

`floor_mode sweep --rx batch|streaming|both` measures the production batch and
streaming receive paths on the same seeds, but only for the `ofdm:1` baseline
until a floor waveform is selected. `connect` and `ramp` need the production
floor session and do not exist yet.

The candidate's batch receiver searches seven CFO hypotheses in 50 Hz steps
between -150 and +150 Hz, sharing a 22-tone matched-filter bank. Decode work
is limited to 16 timing/frequency candidates across the whole search. The
transmitted tone frequencies and frame bytes stay unchanged. See the
[remaining acquisition limits](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-not-built-yet).
Clean SSB tests must flush the FIR tail. Truncating at the original frame
length drops delayed payload samples and measures truncation as well as filtering.
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
