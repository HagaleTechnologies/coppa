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
<<<<<<< HEAD
verified:
  commit: 9295659
  date: 2026-11-11
=======
  - crates/coppa-protocol/src/modem/floor.rs
verified:
  commit: 19948e9f8eb8e8139fb86b9237112cc83ed6095a
  date: 2026-11-12
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b
links:
  - coppa-protocol
  - band-conventions
---
# What has the low-rate floor experiment measured?

COP-187's batch header repair reduces experimental candidate errors on paired
<<<<<<< HEAD
development seeds. It does not select a production waveform. The remaining
Poor failures still violate the separate acquisition bar, despite a favorable
FER crossing. See [measurements](../../BENCHMARKS.md#cop-187-experimental-header-repair)
and [unmet requirements](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-not-built-yet).
=======
development seeds. COP-190 then moved the measured 16-FSK design into the
production crates as air-protocol version 2; no session uses it yet. The
remaining Poor failures on the frozen bench copy still violate the separate
acquisition bar, despite a favorable FER crossing. See
[measurements](../../BENCHMARKS.md#cop-187-experimental-header-repair)
and [open requirements](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-session-integration-open).
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b

Joint length/CRC decoding uses redundancy that independent hard nibble decisions
discard. A legal header hypothesis alone cannot justify delivery. Preserve the
version check, evidence threshold, payload CRC and padding checks, and continue
to count failed headers in SyncFailed.

The Capture adapter bounds history but requires an explicit batch decode.
COP-189's separate `CandidateStream` decodes overlapping windows once per second
and suppresses delivery of the same frame from successive windows. The
`floor_candidate --streaming` diagnostic uses it. A caller must supply trailing
audio through the next one-second decode interval. Work and retention bounds,
and the remaining production gap, are in
<<<<<<< HEAD
[LIMITATIONS](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-not-built-yet).
The experimental header value 2 does not allocate a production air version.
=======
[LIMITATIONS](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-session-integration-open).
COP-190 allocates the header's version value 2 as production air-protocol
version 2 (SPEC §0.3.3, ADR-014); the frozen v1 bytes, host fields and C ABI
are unchanged.
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b

Run the long noise regression explicitly with
`cargo test --release -p coppa-bench --lib continuous_acquisition_300_seconds_noise -- --ignored`.
It covers seeded white noise, independently SSB-filtered one-second noise blocks
and a 1 kHz tone plus noise. It does not establish RF false-alarm probability
or a fading FER crossing.

`floor_mode sweep --rx batch|streaming|both` measures the production batch and
<<<<<<< HEAD
streaming receive paths on the same seeds, but only for the `ofdm:1` baseline
until a floor waveform is selected. `connect` and `ramp` need the production
floor session and do not exist yet.
=======
streaming receive paths on the same seeds, but only for the `ofdm:1`
baseline; sweeping the production floor waveform through the diagnostic is
COP-191's driver work. `connect` and `ramp` need the production floor
session and do not exist yet.
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b


CFO search in the experimental batch and continuous receiver uses thirteen
hypotheses from -150 to +150 Hz in 25 Hz steps. Its matched-filter recurrence
must use separate incoming and outgoing phases: half-bin templates repeat every
two symbols. Reusing the old one-symbol recurrence breaks the +/-75 Hz cases.
The bound is now sixteen sync candidates per hypothesis, or 208 per window.
See [the regression and remaining work](../../BENCHMARKS.md#cop-189-cfo-acquisition-repair).
<<<<<<< HEAD
This search does not supply production framing, negotiation or fading tracking.
=======
COP-190 carries this search into the production floor receiver
(`coppa-protocol::modem::floor`); session negotiation and fading tracking
are still open.

Clean SSB tests must flush the FIR tail. Truncating at the original frame
length drops delayed payload samples and measures truncation as well as filtering.


COP-190 production development sweeps now exercise both floor receive paths
on identical persistent-channel captures. Favorable crossings still fail the
sync-share clause. See [the production measurements](../../BENCHMARKS.md#cop-190-production-floor-development-measurements)
for seeds, accounting and remaining acceptance work. Session integration stays
open; a decoded floor frame does not prove a connection.
>>>>>>> 25a7059f41927da63e57daaa780b4391a919db4b
