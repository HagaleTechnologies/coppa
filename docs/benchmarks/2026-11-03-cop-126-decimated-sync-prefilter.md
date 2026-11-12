> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-126: decimated sync pre-filter on the streaming receive path

`StreamingReceiver` (the daemon, `coppa rx` and the FFI) now builds its
sync detector with `SyncDetector::new_prefiltered`: a 121-tap 250-2850 Hz
band-pass decimated by 4 ahead of the Schmidl-Cox plateau search, with the
confirm and timing steps still at 48 kHz on raw samples
(`docs/adr/011-cop126-decimated-sync-prefilter.md`). This section measures
the sync threshold of both front ends on the input the streaming receiver
sees: full-band white noise with no RX filter.

Reproduce:

```sh
cargo run --release -p coppa-bench --example streaming_sync_threshold
```

The base seed is `12648430` (`0x00C0FFEE`), with 60 trials per point; trial
`i` uses noise seed `0x00C0FFEE + i`. The SNR uses the same
`coppa_channel::awgn_ref_seeded` convention as the other level sweeps: the
whole clean frame's mean power is compared with noise in a 3 kHz reference
bandwidth, and white noise is added across the 24 kHz Nyquist band. Each
trial stream is 4 symbols of silence, the first 6 symbols of an
`hf_standard` frame (preamble, probe and header) and 2 symbols of silence.
A detection is correct when the first candidate lies within 60 samples of
the candidate the same front end finds on the noise-free stream, which
separates sync from FEC. "raw" is `SyncDetector::new`, the front end the
streaming receiver used before COP-126.

Output of the committed program (release build, this container):


### Level 1

| SNR (dB) | 1.5 | 2 | 2.5 | 3 | 3.5 | 4 | 4.5 | 5 | 5.5 | 6 | 6.5 | 7 | 11.5 | 11.75 | 12 | 12.25 | 12.5 | 12.75 | 13 | 13.25 | 13.5 | 13.75 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| raw | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5 | 25 | 45 | 58 | 60 | 60 | 60 | 60 | 60 |
| prefiltered | 10 | 19 | 36 | 47 | 57 | 59 | 60 | 60 | 59 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 |

| front end | 50% point (dB) | 90% point (dB) | gain at 50% (dB) | gain at 90% (dB) |
|---|---|---|---|---|
| raw | 12.31 | 12.67 | 0.00 | 0.00 |
| prefiltered | 2.32 | 3.35 | 9.99 | 9.32 |

### Level 2

| SNR (dB) | 1.5 | 2 | 2.5 | 3 | 3.5 | 4 | 4.5 | 5 | 5.5 | 6 | 6.5 | 7 | 11.5 | 11.75 | 12 | 12.25 | 12.5 | 12.75 | 13 | 13.25 | 13.5 | 13.75 | 14 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| raw | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 4 | 19 | 43 | 56 | 60 | 60 | 60 | 60 | 60 |
| prefiltered | 9 | 18 | 36 | 46 | 56 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 | 60 |

| front end | 50% point (dB) | 90% point (dB) | gain at 50% (dB) | gain at 90% (dB) |
|---|---|---|---|---|
| raw | 12.36 | 12.71 | 0.00 | 0.00 |
| prefiltered | 2.33 | 3.40 | 10.03 | 9.31 |

### Cost

raw: SyncDetector::push realtime factor 0.00788x (10 s of noise, 512-sample chunks, best of 5)
prefiltered: SyncDetector::push realtime factor 0.00557x (10 s of noise, 512-sample chunks, best of 5)

The pre-filter moves the 50% sync point down by about 10 dB at both levels
(9.3 dB at the 90% point), more than the 6 dB the ticket asked for. The
pre-filtered detector also costs about 30% less per sample than the raw
one, because the plateau search runs on a quarter of the samples. Neither
front end changes what the batch path does: `CoppaTransceiver` still
filters with its 601-tap `rx_bpf` and calls `SyncDetector::detect_all`,
which uses `new`.

Prototype measurements from the COP-126 plan phase, not re-run on the
committed code: on `hf_standard` with full-band AWGN, 20 frames per cell
fed in 4800-sample chunks, the streaming receiver decoded 0/20 at 3, 6 and
10 dB for levels 1 to 7 before the change, and matched or beat the batch
API in every cell after it (for example 20/20 at 6 dB for levels 1 to 6).
The golden-vector candidates were sample-identical between the two front
ends on every HF vector, including both Watterson-Poor vectors (`L1_poor25`
at 367, `L2_poor25` at 274). 300 s each of white noise, SSB-band noise and a
1 kHz tone with noise gave no candidate from either front end. This is an
AWGN simulation, with no Watterson sweep of the streaming path and no
hardware or on-air result.

### `--cfo ±150` sweep (AWGN + ±150 Hz carrier offset, level 2)

COP-128, 2026-11-04. `hf_standard`, seed `0x00C0FFEE`, 20 trials per cell,
3 kHz reference-band SNR. The hypothesis search is per-profile and independent
of speed level; this table measures level 2 only. Reproduce each sign with:

```sh
cargo run --release -p coppa-bench -- --level 2 --profile standard --cfo 150 --trials 20 --snr-min 0 --snr-max 18 --snr-step 3 --out-dir /tmp/cop128-plus
cargo run --release -p coppa-bench -- --level 2 --profile standard --cfo -150 --trials 20 --snr-min 0 --snr-max 18 --snr-step 3 --out-dir /tmp/cop128-minus
```

| SNR (dB) | +150 Hz errors/trials | −150 Hz errors/trials | +150 Hz goodput (bps) | −150 Hz goodput (bps) |
|---|---|---|---|---|
| 0.0 | 20/20 | 20/20 | 0.00 | 0.00 |
| 3.0 | 5/20 | 5/20 | 514.29 | 514.29 |
| 6.0 | 1/20 | 1/20 | 651.43 | 651.43 |
| 9.0 | 0/20 | 1/20 | 685.71 | 651.43 |
| 12.0 | 0/20 | 0/20 | 685.71 | 685.71 |
| 15.0 | 0/20 | 0/20 | 685.71 | 685.71 |
| 18.0 | 0/20 | 0/20 | 685.71 | 685.71 |

All observed errors were sync failures. Wilson 95% FER intervals are
[83.89%, 100%] for 20/20 errors, [11.19%, 46.87%] for 5/20,
[0.89%, 23.61%] for 1/20 and [0%, 16.11%] for 0/20. Twenty trials
cannot establish either a 10% or a 1% FER ceiling at 95% confidence.
At 12 dB both signs decoded all 20 frames. The CI guard
`waterfall_cfo150_level_2` allows at most one error per 20-trial sign.

