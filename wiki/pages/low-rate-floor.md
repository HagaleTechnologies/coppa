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
  - crates/coppa-bench/diagnostics/floor_candidate.rs
verified:
  commit: 9295659
  date: 2026-11-11
links:
  - coppa-protocol
  - band-conventions
---
# What has the low-rate floor experiment measured?

COP-187's batch header repair reduces experimental candidate errors on paired
development seeds. It does not select a production waveform. The remaining
Poor failures still violate the separate acquisition bar, despite a favorable
FER crossing. See [measurements](../../BENCHMARKS.md#cop-187-experimental-header-repair)
and [unmet requirements](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-not-built-yet).

Joint length/CRC decoding uses redundancy that independent hard nibble decisions
discard. A legal header hypothesis alone cannot justify delivery. Preserve the
version check, evidence threshold, payload CRC and padding checks, and continue
to count failed headers in SyncFailed.

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
