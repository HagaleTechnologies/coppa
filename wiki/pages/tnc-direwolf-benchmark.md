---
id: tnc-direwolf-benchmark
title: What will bite you when measuring coppa's TNC against Direwolf?
kind: gotcha
status: current
maintainer: agent
sources:
  - tools/tnc-bench/run.sh
  - crates/coppa-bench/src/afsk_direwolf.rs
  - crates/coppa-codec/src/afsk.rs
  - docs/testing/tnc-direwolf-wa8lmf.md
  - docs/testing/tnc-fx25-direwolf.md
  - tools/fx25-interop/run.sh
verified:
  commit: 25ee0e0
  date: 2026-11-05
links:
  - overview
---
The method and the latest counts are in
[tnc-direwolf-wa8lmf.md](../../docs/testing/tnc-direwolf-wa8lmf.md); how to run it is in
[tools/tnc-bench/README.md](../../tools/tnc-bench/README.md).

- The ISO's `02_100-Mic-E-Bursts-DE-emphasized.flac` is CD track 2, the traffic recording, not
  the bursts. `run.sh` pins it by SHA-256.
- `direwolf` and `atest` choose different default profiles on this file (`A+` and `A`) and
  count differently (1000 and 982). The record uses `direwolf`.
- coppa's demodulator is fixed at 48 kHz. A 44.1 kHz file fed in directly decodes nothing.
- `atest` colours its output even into a pipe, and both programs print payload bytes that are
  not UTF-8. Strip escapes, decode lossily, and count `[0] ` or `[0.N] ` lines only.
- Coppa's count moves by up to 28 packets with the start offset. Compare changes against the
  spread the report prints.
- **FX.25 vectors and audio have separate proofs (COP-114).** See
  [the FX.25 record](../../docs/testing/tnc-fx25-direwolf.md) and
  [regeneration instructions](../../tools/fx25-interop/README.md). Controlled
  damaged-symbol correction and Direwolf's zero-error decode of coppa audio
  prove wire compatibility. They do not measure acquisition on noisy audio.
  The untouched Direwolf WAV needs a 20-sample receive-origin offset with the
  existing fixed-window demodulator.

COP-115 adds automatic IL2P receive and opt-in `coppa tnc --il2p` transmit.
See [the IL2P interoperability record](../../docs/testing/tnc-direwolf-il2p.md).
Compare KISS bytes for identity: Direwolf monitor output truncates long frames,
and its 1.7 FEC status labels IL2P as FX.25. Decode counts alone prove neither
frame identity nor zero corrections.

Staggered receive windows alone do not cover sample-clock drift over long IL2P
packets. The codec regression `maximum_packet_tracks_sample_clock_drift` exercises
a maximum-length packet at both +75 and -75 ppm using linear resampling. This
is clean-audio coverage, not a noisy-channel or live RF result.
