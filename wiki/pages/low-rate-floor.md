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
  - crates/coppa-protocol/src/modem/floor.rs
verified:
  commit: 19948e9f8eb8e8139fb86b9237112cc83ed6095a
  date: 2026-11-12
links:
  - coppa-protocol
  - band-conventions
---
# What has the low-rate floor experiment measured?

COP-187's batch header repair reduces experimental candidate errors on paired
development seeds. COP-190 then moved the measured 16-FSK design into the
production crates as air-protocol version 2. The library Link now uses it
for negotiated sessions; the daemon controller remains open. The
remaining Poor failures on the frozen bench copy still violate the separate
acquisition bar, despite a favorable FER crossing. See
[measurements](../../BENCHMARKS.md#cop-187-experimental-header-repair)
and [open requirements](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-session-integration-open).

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
[LIMITATIONS](../../docs/LIMITATIONS.md#cop-169-robust-low-rate-floor-session-integration-open).
COP-190 allocates the header's version value 2 as production air-protocol
version 2 (SPEC §0.3.3, ADR-014); the frozen v1 bytes, host fields and C ABI
are unchanged.

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

The library floor controller reserves streaming delivery time in addition to
airtime. Keying interrupts capture, so Link breaks floor acquisition history
at each transmission boundary. Otherwise retained samples from before and
after transmission can be mistaken for contiguous audio. Probe recovery also
needs room before idle keepalives. See the
[session policy](../../docs/SPEC.md#negotiated-low-rate-floor-session-policy)
and `crates/coppa-engine/tests/floor_session.rs` for deterministic regressions.

Floor-enabled caller retry delays use the complete callsign ordering that also
chooses the answerer during simultaneous calls. A three-bucket delay based on
the last callsign byte left N0AAA and N0AAD synchronized until retries ran out.
The lexical answerer now waits through two OFDM windows and a floor response
window before retrying OFDM. Floor retries keep their normal response window. The paired-audio regression
covers these formerly equal delays; it does not establish the numerical
connection bar.

Discovery staggering can exceed the shared 30-second handshake expiry. For
N0AAD calling N0AAA, a six-second RTO and 500 ms turnaround delay the first
retry until 31 seconds. Link extends its local Calling timeout beyond that
retry and turnaround; retry exhaustion still ends silent calls. Keep these
bounds together when changing discovery timing. See `Link::run_timers` in
[link.rs](../../crates/coppa-engine/src/link.rs) and the boundary and exhaustion
tests in [link/tests.rs](../../crates/coppa-engine/src/link/tests.rs).
