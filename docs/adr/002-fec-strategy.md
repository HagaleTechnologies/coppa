# ADR-002: FEC Strategy

## Status

Superseded by ADR-005

**COP-146 update (2026-11-11):** neither half of this decision describes the shipped system.
ADR-005 replaced the six-rate 802.11 QC-LDPC family with a single NR BG2 mother code,
rate-matched per speed level. The rate-1/2 K=7 convolutional code and its Viterbi decoder
remain in `coppa-protocol` (`fec/convolutional.rs`), but no transmit or receive path uses
them: `grep -rn 'ViterbiDecoder\|ConvEncoder' crates` finds no use outside that file, and
`ARCHITECTURE.md`'s "Legacy code not on the data path" lists the benchmark and fuzz target
as its only callers. The text below is kept as the original record.

## Context

Forward Error Correction (FEC) is critical for reliable communication over noisy HF channels. The project needs to support multiple FEC schemes at different code rates to adapt to varying channel conditions.

## Decision

Implement two FEC families behind the `FecCodec` trait:

1. **Convolutional codes** (rate 1/2, constraint length K=7) with soft-decision Viterbi decoder — used as the default for BPSK transmissions. Low complexity, well-understood, good performance at moderate SNR.

2. **QC-LDPC codes** (6 rates from 1/4 to 7/8) with offset min-sum belief propagation decoder — used for OFDM modes. Higher complexity, configurable max iterations with early termination. (No measured operating curves are published; LDPC is a strong code family but Coppa makes no specific performance claim.)

Both expose `encode()`/`decode()`; the `FecCodec` trait is intended to let the engine swap FEC schemes based on the selected MCS level (this abstraction is not yet wired through the flagship modems).

Turbo codes are deferred as a future option.

## Consequences

- Two well-tested FEC implementations span a range of code rates intended for a wide range of channel conditions (operating SNR ranges are not empirically characterized)
- The `FecCodec` trait allows new schemes (e.g., turbo codes) to be added without engine changes
- LDPC codes use 1,944 coded bits at all rates, simplifying frame sizing
- Trade-off: LDPC decoder is compute-heavy at low SNR (up to 50 iterations)
