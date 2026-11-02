# Roadmap

Linear is the project's tracker. This file mirrors the open items a reader can check from the
repository, each with the document that records it. It states no dates, targets or performance
promises, per the ethos in [CONTRIBUTING.md](CONTRIBUTING.md). An item leaves this file when the
document it links records it as closed.

## Open measured gaps

Each item quotes its bullet in [docs/LIMITATIONS.md](docs/LIMITATIONS.md), which holds the
measurements and any later updates.

- **Closed-loop rate adaptation.** "Phase 3's closed-loop rate adaptation (Task 4) does not meet
  its own acceptance bar." Log: [Phase 3 Task 4](docs/benchmarks/2026-07-phase3-closed-loop-rate.md).
- **`milstd` and `session` benchmarks.** "The Phase 3 benchmark program (Task 8: `milstd`,
  `session`) does not clear its own acceptance targets." Log:
  [Phase 3 Task 8](docs/benchmarks/2026-07-phase3-milstd-session-golden.md).
- **Block-ACK cadence.** "Decision 4's block-ACK cadence (Phase 3 Task 2) was never
  implemented." Every decoded frame still triggers its own ACK.
- **Per-codeword ACK and IR-HARQ.** "Multi-codeword frames (Phase 3 Task 5) do not extend ACK
  addressing, turbo re-estimation, or persistent IR-HARQ combining to the per-codeword level."
  See [ADR-007](docs/adr/007-multi-codeword-frames.md).
- **Phase 2 channel estimation.** "Phase 2 channel estimation (Task 1 delay-domain estimator +
  Task 7 Kalman tracker) — a real, unresolved regression, shipped anyway." See
  [ADR-006](docs/adr/006-phase2-parametric-estimation-nr-bg2.md).
- **Phase 2 cumulative bar.** "Phase 2's cumulative full-ladder re-baseline (Task 8) does not
  cleanly clear the phase's own acceptance bar." Log:
  [Phase 2 re-baseline](docs/benchmarks/2026-07-phase2-cumulative-rebaseline.md).

## Not started

- **IR-HARQ wiring.** The README [status table](README.md#status) lists IR-HARQ as an "Unwired
  codec primitive": engine and daemon retransmissions do not use the codec's redundancy-version
  and combining support.
- **On-air and two-radio measurement.** No live two-radio field test exists; every performance
  figure is a simulation. See the [COP-24 log](docs/benchmarks/2026-10-23-cop-24-hf-ceiling-ssb.md)
  and [ARCHITECTURE.md](ARCHITECTURE.md)'s "Goals".
- **Sub-0 dB weak-signal operation.** The [COP-67 log](docs/benchmarks/2026-10-28-cop-67-hf-narrow-level-1.md)
  records "the weak-signal target failed" for `hf_narrow` level 1 on AWGN. Lower sync thresholds
  and their false-alarm trade-off were not measured.
- **KISS TNC parity.** The README's [KISS TNC section](README.md#kiss-tnc-afsk-1200) lists what
  the AFSK 1200 TNC lacks (AGWPE, channel-busy check, digipeating, other baud rates) and points
  to H-122 in the 2026-09-05 product review.

## On hold

- **Documentation site** (GitHub Pages or mdBook, review item H-118) and **crates.io name
  registration** (H-119). Both are public-facing and wait for an owner call. Until then, the
  crates are used from git; see the [embedding guide](docs/tutorials/embedding.md).

## In flight

Work under way is in the [open pull requests](https://github.com/HagaleTechnologies/coppa/pulls).
A draft pull request is how a contributor claims an item (see [CONTRIBUTING.md](CONTRIBUTING.md)).
