---
id: tnc-g3ruh-9600
title: What will bite you about the 9600 baud G3RUH TNC?
kind: gotcha
status: current
maintainer: agent
sources:
  - crates/coppa-codec/src/g3ruh.rs
  - docs/TNC.md
  - docs/testing/tnc-direwolf-g3ruh-9600.md
verified:
  commit: 7b00317
  date: 2026-11-04
links:
  - tnc-direwolf-benchmark
  - overview
---
Select `coppa tnc --baud 9600`; the default remains 1200.
The radio needs a discriminator or 9600-capable direct-FSK connection.
See [TNC.md](../../docs/TNC.md) for wiring and the
[measurement record](../../docs/testing/tnc-direwolf-g3ruh-9600.md) for the fixture,
settings, counts and reproduction steps.

The receiver merges phase-bank copies within 150 ms, so identical real repeats
inside that window can also be suppressed. Fixed phases have no clock tracking.
The pinned synthetic fixture is an offline acceptance test, not an RF benchmark.
