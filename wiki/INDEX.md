# coppa wiki index

- [Coppa — what is this and where do things live?](pages/overview.md) — Coppa is an open-source OFDM digital modem for amateur radio written in Rust,
- [How does coppa-dsp work and what does it expose?](pages/coppa-dsp.md) — `coppa-dsp` is the pure-DSP foundation of the workspace — no audio I/O, no async,
- [How does coppa-engine orchestrate the modem pipeline?](pages/coppa-engine.md) — `coppa-engine` is a thin ~210-line wrapper (`CoppaCore`) around `CoppaTransceiver`,
- [How does coppa-protocol handle framing, FEC, and ARQ?](pages/coppa-protocol.md) — `coppa-protocol` contains the full protocol stack above the physical layer:
- [Why is coppa a 12-crate Cargo workspace?](pages/adr-001-workspace-layout.md) — The codebase was restructured from a single crate into 12 crates under `crates/`
- [Why does coppa use both convolutional and LDPC codes?](pages/adr-002-fec-strategy.md) — Two FEC families cover the range of channel conditions: rate-1/2 K=7
- [Why does Phase 1 break the waveform and what changed?](pages/adr-003-waveform-break.md) — Phase 1 ("radio reality") moved the waveform from a full-Nyquist band (starting
- [Why does SyncDetector prefer the strongest multipath tap, not the first?](pages/adr-004-strongest-path-timing.md) — Phase 1's `SyncDetector` originally anchored sync timing on the first-arriving
- [What will bite you about the cpal-backend feature flag?](pages/cpal-feature-gate.md) — The daemon (`coppad`) compiles and runs without the `cpal-backend` feature flag,
- [What will bite you about LDPC at speed levels 9 and 10?](pages/ldpc-non-convergence.md) — Speed levels 9 and 10 (64-QAM with 7/8 and 3/4 LDPC) fail to decode reliably
- [What will bite you about Watterson fading at speed level 4?](pages/watterson-level-4-gap.md) — Speed level 4 (QPSK 3/4 with sparse-pilot `hf_standard`/`hf_wide`/`hf_narrow`
- [What will bite you about waveform compatibility with pre-Phase-1 code?](pages/waveform-wire-break.md) — The Phase 1 waveform is a hard wire-format break from all earlier coppa revisions.
- [What band and sample-rate conventions does coppa use?](pages/band-conventions.md) — Coppa operates at a fixed 48 kHz sample rate across all profiles and speed
- [What contract does coppa-dsp expose to skimmer?](pages/coppa-dsp-skimmer-interface.md) — `coppa-dsp` is consumed by the skimmer repo as a pure-DSP library — it provides
