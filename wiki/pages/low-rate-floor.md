---
id: low-rate-floor
<<<<<<< HEAD
title: What has the low-rate floor experiment measured?
=======
title: What has COP-169 measured about a low-rate floor?
>>>>>>> cddfc25087a370f4b7178ed48f234674c26ecf38
kind: gotcha
status: current
maintainer: agent
sources:
  - BENCHMARKS.md
  - docs/LIMITATIONS.md
  - crates/coppa-bench/src/floor_candidate.rs
<<<<<<< HEAD
verified:
  commit: 9295659
=======
  - crates/coppa-bench/diagnostics/floor_candidate.rs
verified:
  commit: aa4bf43
>>>>>>> cddfc25087a370f4b7178ed48f234674c26ecf38
  date: 2026-11-11
links:
  - coppa-protocol
  - band-conventions
---
<<<<<<< HEAD
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
=======
# What has COP-169 measured about a low-rate floor?

COP-169 has a measurement contract and an experimental 16-FSK batch receiver.
It has no production floor mode. See [BENCHMARKS](../../BENCHMARKS.md#cop-169-experimental-16-fsk-candidate)
for the development counts and commands, and [LIMITATIONS](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-not-built-yet)
for unmet session requirements.

A favorable FER crossing alone does not select a waveform. The Poor development
sweep crossed below the requested threshold but failed the separate acquisition
bar because its remaining failures were in the header. Keep these failures in
SyncFailed; excluding them would make the prototype appear to pass.

The bounded Capture adapter tests chunk partitioning. Its caller asks for batch
decode after capture, so it is not evidence of continuous streaming acquisition.
The experimental header value does not allocate a production air version.
>>>>>>> cddfc25087a370f4b7178ed48f234674c26ecf38
