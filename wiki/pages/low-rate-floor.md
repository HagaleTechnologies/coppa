---
id: low-rate-floor
title: What has COP-169 measured about a low-rate floor?
kind: gotcha
status: current
maintainer: agent
sources:
  - BENCHMARKS.md
  - docs/LIMITATIONS.md
  - crates/coppa-bench/src/floor_candidate.rs
  - crates/coppa-bench/diagnostics/floor_candidate.rs
verified:
  commit: aa4bf43
  date: 2026-11-11
links:
  - coppa-protocol
  - band-conventions
---
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
