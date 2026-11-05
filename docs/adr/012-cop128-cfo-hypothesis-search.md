# ADR-012: Fine-stage CFO hypothesis search

## Status

Accepted, COP-128, 2026-11-04.

## Context

H-101 from COP-87 identified the one-subcarrier acquisition limit. At 48 kHz
with a 960-point FFT the coarse Moose lag of 480 samples wraps beyond ±50 Hz.
Research found a hard decode envelope near ±48 Hz, even without noise.
At +150 Hz the two-stage estimate was about −40.5 Hz, wrong by five fine
ambiguity steps. Manually correcting offsets let the existing receiver
decode from −200 to +200 Hz at 12 dB. The missing capability was ambiguity
resolution in acquisition, rather than demodulation or FEC.

A wrong estimate can still confirm-correlate around 0.75 to 0.81, far above
the 0.25 acceptance threshold. Accepting the first hypothesis over threshold
would preserve the failure.

## Decision

Retain the fine lag-symbol_len Moose estimate from the plateau peak's
analytic ring window. Search `fine + j * Δ`, for `j` from −4 to +4 and
`Δ = sample_rate / symbol_len`. Choose the global maximum normalized confirm
cross-correlation, breaking ties within 1e-6 by smaller `|j|`. Apply the
existing confirmation threshold and strongest/first-path timing rule to
the winner. The candidate carries that winning frequency to whole-buffer
CFO removal in both batch and streaming receivers.

Compute the confirm window's analytic signal once. Each hypothesis needs
only rotation and correlation. Preserve the absolute sample phase origin.
Decimation divides both the symbol length and sample rate, so Δ is invariant.
The public two-stage utility remains available with its existing tests;
acquisition uses the shared crate-private fine estimator instead.

| Profile | Fine wrap (Hz) | Δ (Hz) | Search envelope (Hz) |
|---|---|---|---|
| standard / robust / narrow / wide | ±19.05 | 38.10 | ±171.4 |
| standard short-CP | ±21.74 | 43.48 | ±195.7 |
| VHF wide | ±23.53 | 47.06 | ±211.8 |

These are search bounds, not a guarantee under every channel impairment.
There is no TX, public API, or wire-format change. SPEC §3.1 is untouched.

## Rejected alternative

A shorter coarse lag such as N/6 would need a different preamble comb
congruence class. It breaks the existing N/2 periodicity assumption and
requires a wire-format change with an interop cliff. Searching around the
old two-stage estimate is also unsuitable: its five-step error at +150 Hz
would escape a four-step search. Centering on the wrapped fine estimate
provides the coverage bound above.

## Measurements and verification

Before the change, `loopback_decodes_distant_cfo_offsets` failed at −150 Hz
with `SyncFailed`, `streaming_decodes_distant_cfo_offsets` returned zero
frames, and `waterfall_cfo150_level_2` lost 20/20 frames with sync failures.
Both detector regressions returned about +40.5 Hz for true −150 Hz, scoring
0.809 unfiltered and 0.810 prefiltered. All existing CFO-filtered tests passed.
These are the recorded Red results on the unchanged receiver.

After the change, batch loopback decodes −150/−100/−60/+60/+100/+150 Hz,
both clean and with seeded 12 dB AWGN, with matching payload. Streaming
decodes both ±150 Hz seeded-AWGN frames. The shipped bench guard passes.
BENCHMARKS.md's COP-128 table records the CLI sweep: both signs lose 0/20
frames at 12 dB, with 685.71 bps goodput.

The committed detector tests print negative-side observations at 20 dB:

| CFO (Hz) | Identity estimate / confirm | Prefiltered estimate / confirm |
|---|---|---|
| −150 | −149.990 / 0.99102 | −149.967 / 0.99060 |
| −100 | −99.985 / 0.99066 | −99.997 / 0.99077 |
| −60 | −59.974 / 0.99092 | −59.979 / 0.99101 |

Negative offsets produce candidates on both paths. The earlier research's
zero-candidate inference is not universal: the leading-silence regression
fixture emitted a wrong-frequency candidate before the fix. The search
repairs ambiguity selection in either case.

The non-gating `hf_narrow` probe at −150 Hz, 20 dB, seed `0xBEEF`, resolves
−150.053 Hz with confirm 0.99156 despite some carriers crossing the streaming
prefilter's 250 Hz low edge. This measures detection only, not narrow-profile
payload performance across the whole envelope.

`cargo run --release -p coppa-protocol --example streaming_perf` measured
[1] noise search at 65.902 ms per 10 seconds, or 0.006590× realtime, below
the COP-128 budget of 0.011×. It measured full-frame decode at 122.342 ms;
the example's older 5 ms target is not met. The nine-correlation search
adds candidate-time work; this run does not claim a frame-latency improvement.
No early-exit fallback was needed for the noise-search budget.
