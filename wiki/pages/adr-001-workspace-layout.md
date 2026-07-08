---
id: adr-001-workspace-layout
title: Why is coppa a 12-crate Cargo workspace?
kind: decision-digest
status: current
maintainer: agent
sources:
  - docs/adr/001-workspace-layout.md
verified:
  commit: c1d2676
  date: 2026-07-07
links:
  - overview
---
The codebase was restructured from a single crate into 12 crates under `crates/`
to allow independent compilation, testing, and feature-gating of each layer. The
key insight: `coppa-dsp` needs no async, no audio, no FFI; `coppa-channel` is
test-only; `coppa-audio` needs CPAL which is optional. A flat monolith forces
all these into one compile graph. The trade-off is more `Cargo.toml` files and
a longer initial full-workspace compile.

## Digest

The workspace is organized by layer (DSP → Codec → Protocol → Engine → Interface
→ Application), with `coppa-channel` as test-only infrastructure. Feature flags
(`cpal-backend`, `file-backend`, `websocket`) gate optional heavyweight
dependencies without affecting the core crates.

Full rationale: `docs/adr/001-workspace-layout.md`
