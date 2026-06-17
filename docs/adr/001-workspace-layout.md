# ADR-001: Workspace Layout

## Status

Accepted

## Context

The original Coppa codebase was a single crate with all functionality in `src/`. As the project grew to include OFDM, LDPC FEC, audio I/O, radio control, host interfaces, and FFI bindings, the monolithic layout became difficult to maintain and test independently.

## Decision

Restructure Coppa as a Cargo workspace with 12 crates under `crates/`, organized by layer:

- **DSP layer**: `coppa-dsp` (pure signal processing)
- **Codec layer**: `coppa-codec` (modulation/demodulation, depends on dsp)
- **Protocol layer**: `coppa-protocol` (framing, FEC, ARQ, depends on codec)
- **Hardware layer**: `coppa-audio`, `coppa-radio` (I/O, independent)
- **Adaptation layer**: `coppa-ml` (channel prediction via EWMA + MCS selection, independent)
- **Integration layer**: `coppa-engine` (orchestration, depends on codec+protocol)
- **Interface layer**: `coppa-host` (TCP/WebSocket, depends on engine)
- **Application layer**: `coppa-cli`, `coppa-daemon`, `coppa-ffi` (binaries/library)
- **Test infrastructure**: `coppa-channel` (channel models for testing)

The root `Cargo.toml` defines workspace-level dependencies and a root package for integration tests and benchmarks.

## Consequences

- Each crate compiles and tests independently, improving CI times for targeted changes
- Clear dependency graph prevents circular dependencies
- Feature flags (e.g., `cpal-backend`, `file-backend`, `websocket`) allow optional heavyweight dependencies
- Trade-off: more Cargo.toml files to maintain and longer initial full-workspace compile
