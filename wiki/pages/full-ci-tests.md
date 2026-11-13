---
id: full-ci-tests
title: What keeps full CI coverage intact when splitting optimized tests?
kind: gotcha
status: current
maintainer: agent
sources:
  - docs/testing/full-ci.md
  - tools/ci/run-full-tests.sh
  - .github/workflows/ci-full.yml
  - crates/coppa-host/src/vara/command.rs
verified:
  commit: fa4de8e
  date: 2026-11-13
links:
  - paused-clock-timers
  - cpal-feature-gate
---
Full CI uses a test-derived optimized profile that retains debug assertions and
overflow checks. See [the commands and measurements](../../docs/testing/full-ci.md)
before changing group boundaries or claiming a hosted performance result.

The no-audio group is separate coverage. Enabling CPAL compiles out some daemon
process tests. Cargo rejects the daemon-only mdns feature when that package is
excluded from the workspace group.

The dropped-file-chunk test runs with a paused Tokio clock. Its 29.5/30-second
sleeps advance virtual time; they do not demonstrate minutes of wall-clock
waiting. Unpaused event-loop RTO helpers do sleep on the real thread. Preserve
that distinction when profiling simulations.

A paused Tokio deadline can outrun the real TCP delayed-ACK timer. Full-CI
measurement exposed this in BPQ startup replies; the VARA command server
disables Nagle buffering so small replies do not wait for real-time ACKs.
See the measurement document for the baseline reproduction and fix.
