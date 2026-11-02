> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-67: `hf_narrow` level-1 AWGN feasibility

This simulation checks whether the existing 8-data-carrier, 2-pilot-carrier
`hf_narrow` profile with level 1 (BPSK, LDPC 1/4) reaches FER ≤ 10% below
0 dB. It uses the batch `CoppaTransceiver::receive` path; its waveform and
receiver code is unchanged from COP-67 base commit
`5525bb62613a9205daa10e963538bb720e800563`. Reproduce the sweep with:

```sh
cargo build --release -p coppa-bench
target/release/coppa-bench --channel awgn --profile narrow --level 1 --snr-min -6 --snr-max 4 --snr-step 1 --trials 200 --seed 12648430 --out-dir results/cop-67-hf-narrow
```

The base seed is `12648430` (`0x00C0FFEE`), with 200 independent frames at
each of eleven points. Raw counts and failure categories are in
`results/cop-67-hf-narrow/awgn.csv`. FER is `frame_errors / trials`. The SNR
uses the same `coppa_channel::awgn_ref_seeded` convention as the existing
level sweeps: the clean transmitted frame's mean power (including preamble,
probe, and header) is compared with noise in a 3 kHz reference bandwidth;
white noise is added across the 24 kHz Nyquist band. A claim of FER ≤ 10%
also requires the 95% Wilson **upper** bound to be ≤ 0.10.

| SNR (dB) | Frame errors / 200 | FER | 95% Wilson interval |
|---:|---:|---:|---:|
| −6 | 200 | 1.000 | 0.981–1.000 |
| −5 | 200 | 1.000 | 0.981–1.000 |
| −4 | 200 | 1.000 | 0.981–1.000 |
| −3 | 200 | 1.000 | 0.981–1.000 |
| −2 | 200 | 1.000 | 0.981–1.000 |
| −1 | 200 | 1.000 | 0.981–1.000 |
| 0 | 200 | 1.000 | 0.981–1.000 |
| +1 | 198 | 0.990 | 0.964–0.997 |
| +2 | 182 | 0.910 | 0.862–0.942 |
| +3 | 112 | 0.560 | 0.491–0.627 |
| +4 | 33 | 0.165 | 0.120–0.223 |

**Result: the weak-signal target failed.** Every frame at every measured
subzero point was lost; even +4 dB has FER 16.5% (Wilson upper bound 22.3%).
The benchmark's `SyncFailed` category accounts for these losses, but that
category also includes protected-header decode failures after candidate
detection, so it does not by itself locate the limit. A separate probe in
the COP-67 research ran 100 trials per point through the same batch receive
filter and found no sync candidate at subzero SNR; when a correctly timed
candidate existed, it observed no header or payload failures. The
profile-independent sync detector's plateau and confirm thresholds are a
plausible mechanism, but lower thresholds and their false-alarm trade-off
were not measured. No operator profile was enabled. This is an AWGN batch
simulation, with no hardware or on-air result.
