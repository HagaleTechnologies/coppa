> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-133: Same-frame residual experiment

2026-11-04. Partial implementation; the rate-loop bar remains unmet. The candidate
was not adopted. Production recommendations, controller settings, waveforms,
LDPC noise vectors, SNR telemetry and comparator definitions are unchanged.

The checkout was reconciled from `02650835d98c6519c7fdb4a12693af6523717874`
to main `b8af910712b134f1dec2e0e645cdbe8a49005ba3` before the retained runs.
The only production refactor extracts the existing payload-to-constellation chain
into a private helper also called by transmit. A frozen copy of the original
chain verifies identity at all nine levels, RVs 0 through 3, codeword counts
1, 2 and 8, and empty, one-byte and full payloads.

### Applicability experiment

Command:

```sh
cargo test -p coppa-protocol --release --lib residual_recommendation_grid -- --ignored --nocapture
```

Training base `0xCA11B`, held-out base `0x5A1AD`; eight seeds per base, all nine
measured levels, AWGN and Watterson Good/Moderate/Poor, SNR grid 6/12/18/24/30 dB,
`hf_robust`. Each trial evicts HARQ state. Reconstruction uses the CRC-verified
payload and current RV. Turbo rescues fall back because the experiment does not
retain their re-equalized observations. Residual pooling follows the alternating
physical-carrier map, excludes unused padding and rejects missing coded positions.
Only capacity conversion floors residual variance at 1e-6. No pilot PAPR-bias
correction or new calibration constants are applied.

All 2,880 receive attempts completed. No successful probe provided complete
observations, so every recommendation fell back to the pilot result. For example,
level 1 reconstructs 1,944 positions but frame-sized HF input yields only 1,908
equalized positions after RX bandpass filtering. A deterministic normal test
reproduces this and shows that test-only trailing receive context restores the
complete geometry. The production capture and decoder are unchanged.

The following paired records are grouped by base, preset and measured level.
`ok/fail` counts CRC-successful probes and failed receive attempts out of 40.
`bps` is mean delivered payload bits divided by the chosen level's transmitted
sample duration, conditional on successful probe decoding. Candidate and pilot
values are identical. `regret` is the mean difference from the same-realization
nine-level oracle in those units; `miss` counts choices with lower goodput than
that oracle. These conditional observations are not the closed-loop acceptance
metric. Complete residual observations and changed recommendations are both zero
in every row. No claim of improved recommendation accuracy or next-frame feedback
value follows from these data.

| Base | Preset | Measured level | ok/fail | Pilot = candidate bps | Regret bps | miss |
|---|---|---:|---:|---:|---:|---:|
| ca11b | AWGN | 1 | 40/0 | 3381.406 | 156.916 | 10 |
| ca11b | AWGN | 2 | 40/0 | 3381.406 | 156.916 | 10 |
| ca11b | AWGN | 3 | 40/0 | 3224.490 | 313.832 | 16 |
| ca11b | AWGN | 4 | 40/0 | 3224.490 | 313.832 | 16 |
| ca11b | AWGN | 5 | 40/0 | 3010.431 | 527.891 | 27 |
| ca11b | AWGN | 6 | 40/0 | 3086.621 | 451.701 | 23 |
| ca11b | AWGN | 7 | 40/0 | 3010.431 | 527.891 | 27 |
| ca11b | AWGN | 8 | 36/4 | 3036.735 | 607.710 | 28 |
| ca11b | AWGN | 9 | 30/10 | 3127.438 | 643.991 | 22 |
| ca11b | Good | 1 | 40/0 | 609.137 | 765.757 | 22 |
| ca11b | Good | 2 | 40/0 | 569.249 | 805.645 | 23 |
| ca11b | Good | 3 | 21/19 | 469.732 | 974.442 | 20 |
| ca11b | Good | 4 | 15/25 | 657.625 | 944.723 | 14 |
| ca11b | Good | 5 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Good | 6 | 10/30 | 848.980 | 1203.084 | 9 |
| ca11b | Good | 7 | 4/36 | 1697.959 | 885.261 | 4 |
| ca11b | Good | 8 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Good | 9 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Moderate | 1 | 40/0 | 481.035 | 558.745 | 24 |
| ca11b | Moderate | 2 | 35/5 | 493.084 | 655.267 | 25 |
| ca11b | Moderate | 3 | 14/26 | 752.301 | 844.114 | 12 |
| ca11b | Moderate | 4 | 6/34 | 699.160 | 896.359 | 6 |
| ca11b | Moderate | 5 | 5/35 | 1743.586 | 182.509 | 4 |
| ca11b | Moderate | 6 | 10/30 | 848.980 | 963.048 | 10 |
| ca11b | Moderate | 7 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Moderate | 8 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Moderate | 9 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Poor | 1 | 33/7 | 178.627 | 587.163 | 23 |
| ca11b | Poor | 2 | 23/17 | 364.467 | 612.630 | 13 |
| ca11b | Poor | 3 | 10/30 | 292.272 | 1195.146 | 10 |
| ca11b | Poor | 4 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Poor | 5 | 5/35 | 1697.959 | 228.136 | 5 |
| ca11b | Poor | 6 | 5/35 | 1697.959 | 228.136 | 5 |
| ca11b | Poor | 7 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Poor | 8 | 0/40 | 0.000 | 0.000 | 0 |
| ca11b | Poor | 9 | 0/40 | 0.000 | 0.000 | 0 |
| 5a1ad | AWGN | 1 | 40/0 | 3381.406 | 207.937 | 14 |
| 5a1ad | AWGN | 2 | 40/0 | 3400.454 | 188.889 | 13 |
| 5a1ad | AWGN | 3 | 40/0 | 3262.585 | 326.757 | 18 |
| 5a1ad | AWGN | 4 | 40/0 | 3243.537 | 345.805 | 19 |
| 5a1ad | AWGN | 5 | 40/0 | 3010.431 | 578.912 | 30 |
| 5a1ad | AWGN | 6 | 40/0 | 3083.537 | 505.805 | 26 |
| 5a1ad | AWGN | 7 | 40/0 | 3067.574 | 521.769 | 27 |
| 5a1ad | AWGN | 8 | 39/1 | 3001.849 | 613.291 | 31 |
| 5a1ad | AWGN | 9 | 31/9 | 3109.882 | 661.546 | 23 |
| 5a1ad | Good | 1 | 40/0 | 806.831 | 1306.054 | 39 |
| 5a1ad | Good | 2 | 35/5 | 933.126 | 1279.553 | 35 |
| 5a1ad | Good | 3 | 30/10 | 924.576 | 1226.347 | 29 |
| 5a1ad | Good | 4 | 20/20 | 1247.859 | 686.915 | 20 |
| 5a1ad | Good | 5 | 20/20 | 979.505 | 1176.584 | 20 |
| 5a1ad | Good | 6 | 34/6 | 1461.679 | 732.590 | 29 |
| 5a1ad | Good | 7 | 17/23 | 1697.959 | 885.261 | 17 |
| 5a1ad | Good | 8 | 0/40 | 0.000 | 0.000 | 0 |
| 5a1ad | Good | 9 | 0/40 | 0.000 | 0.000 | 0 |
| 5a1ad | Moderate | 1 | 40/0 | 220.435 | 2025.522 | 40 |
| 5a1ad | Moderate | 2 | 35/5 | 435.589 | 1888.653 | 34 |
| 5a1ad | Moderate | 3 | 21/19 | 1178.625 | 976.076 | 19 |
| 5a1ad | Moderate | 4 | 16/24 | 1364.275 | 711.843 | 14 |
| 5a1ad | Moderate | 5 | 20/20 | 1199.978 | 1074.228 | 19 |
| 5a1ad | Moderate | 6 | 34/6 | 1685.266 | 656.595 | 27 |
| 5a1ad | Moderate | 7 | 17/23 | 1697.959 | 885.261 | 17 |
| 5a1ad | Moderate | 8 | 4/36 | 0.000 | 3009.524 | 4 |
| 5a1ad | Moderate | 9 | 0/40 | 0.000 | 0.000 | 0 |
| 5a1ad | Poor | 1 | 35/5 | 756.286 | 1617.457 | 31 |
| 5a1ad | Poor | 2 | 35/5 | 682.529 | 1691.214 | 31 |
| 5a1ad | Poor | 3 | 33/7 | 913.812 | 1302.666 | 28 |
| 5a1ad | Poor | 4 | 19/21 | 864.332 | 1118.503 | 14 |
| 5a1ad | Poor | 5 | 20/20 | 1426.808 | 748.829 | 14 |
| 5a1ad | Poor | 6 | 29/11 | 1924.918 | 541.441 | 22 |
| 5a1ad | Poor | 7 | 19/21 | 2070.701 | 692.016 | 15 |
| 5a1ad | Poor | 8 | 8/32 | 1697.959 | 1311.565 | 8 |
| 5a1ad | Poor | 9 | 0/40 | 0.000 | 0.000 | 0 |

This rejects applicability under the planned complete-observation requirement,
not residual estimation in general. No positive deterministic improvement fixture
was found, so the residual code stays in the private test experiment. It adds no
production receive encode or mapping work, and no receive CPU overhead comparison
is claimed. Reconstructing missing observations by changing the decoder, truncating
the expected stream or changing the benchmark capture would be a different experiment.

### Ground-truth diagnostic correction

```sh
cargo run -p coppa-bench --release --example capacity_ground_truth_diagnosis
```

All three 300-trial presets completed at 24 dB, measured level 1, robust profile.
The historical zero-filled statistics remain in the diagnostic output. Additional
statistics exclude failed demodulations without changing the successful pairs:

| Preset | Demodulated/failed | Capacity correlation | Selectivity correlation |
|---|---:|---:|---:|
| Good | 300/0 | 0.050 | 0.131 |
| Poor | 284/16 | -0.028 | 0.051 |
| Moderate | 299/1 | -0.230 | -0.140 |

The full-frame/second-half windows overlap. Model truth averages all active
carriers while measured noise follows alternating data carriers. The diagnostic
uses the raw modem without the transceiver RX bandpass, and AWGN references each
faded frame's realized power. These method differences and correlations establish
no physical ceiling. Historical raw numbers in LIMITATIONS.md are preserved.

### Full closed-loop controls

Both commands completed with five seeds `[0,1,2,3,4]`, 300 frames per seed,
and no smoke overrides:

```sh
cargo run -p coppa-bench --release --example closed_loop_arq
cargo run -p coppa-bench --release --example closed_loop_arq -- standard
```

These are pre-integration controls. Since the candidate was not adopted, the
final production path has identical recommendations and transmitted symbols.
No separate after-variant run or paired fading-segment goodput is claimed.
All transmitted frames, including failures, remain charged to airtime. The
comparator still contains all 18 level/CP cells. Robust uses a synthetic short-CP
twin, while standard uses the production standard/standard-short-CP profile pair.
The x1 column charges one five-frame handshake per CP switch; x0 is supplemental.
Aggregate ratios are the program's mean adaptive goodput divided by mean
comparator goodput, never means of per-seed ratios. Values below are the program's
rounded output; no unrounded passing verdict is inferred from them.

#### Robust control

| Seed | B delivered | B payload bits | Data air s | Switch air s | B x0 bps | B x1 bps | Joint best fixed bps | Joint oracle bps |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 | 257/300 | 326456 | 173.45 | 7.61 | 1882.1 | 1803.0 | 2250.5 | 3260.0 |
| 1 | 257/300 | 341888 | 160.25 | 7.61 | 2133.5 | 2036.8 | 2230.8 | 3218.3 |
| 2 | 259/300 | 330896 | 189.55 | 7.61 | 1745.7 | 1678.3 | 2338.9 | 3314.6 |
| 3 | 260/300 | 328584 | 171.01 | 7.61 | 1921.4 | 1839.5 | 2309.5 | 3269.8 |
| 4 | 258/300 | 329680 | 178.03 | 7.61 | 1851.9 | 1775.9 | 2221.0 | 3248.4 |

Legacy long-CP arm A control, delivered application bits per 300 frame slots.
These ratios are not comparable to the airtime ratios above.

| Seed | Adaptive bits | Best fixed bits | Oracle bits | a/bf | a/oracle |
|---|---:|---:|---:|---:|---:|
| 0 | 330144 | 358848 | 429256 | 0.920010701 | 0.769107479 |
| 1 | 325680 | 350304 | 427056 | 0.929706769 | 0.762616612 |
| 2 | 318512 | 367392 | 429416 | 0.866954098 | 0.741732958 |
| 3 | 325208 | 350304 | 427552 | 0.928359368 | 0.760627947 |
| 4 | 323888 | 364544 | 430880 | 0.888474368 | 0.751689566 |

Aggregate decision output, x0 then x1:

```text
arm B / best-fixed(joint) = 0.840 (x0) / 0.805 (x1)  (bar: > 1.0)
arm B / oracle(joint)     = 0.585 (x0) / 0.560 (x1)  (bar: >= 0.8)
```

Existing delay-spread segment output, retained as descriptive context only.
It does not measure fading-segment delivered-goodput non-regression.

```text
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up        97       0       0       0       0       0       0       0         3         0
s0 awgn ramp down      88       0       0       0       0       0       0       0        12         0
s0 watterson good      23      10       0       0       0       0       0       0        17         0
s0 watterson poor      23       0       0       0       0      16       0       0        11         0
s1 awgn ramp up        95       0       0       0       0       0       0       0         5         0
s1 awgn ramp down      91       0       0       0       0       0       0       0         9         0
s1 watterson good      27       8       0       0       0       0       0       0        15         0
s1 watterson poor      20       1       0       0       1      14       0       0        14         0
s2 awgn ramp up        97       0       0       0       0       0       0       0         3         0
s2 awgn ramp down      92       0       0       0       0       0       0       0         8         0
s2 watterson good      23       9       0       0       0       0       0       0        18         0
s2 watterson poor      22       0       0       0       0      16       0       0        12         0
s3 awgn ramp up        95       0       0       0       0       0       0       0         5         0
s3 awgn ramp down      90       0       0       0       0       0       0       0        10         0
s3 watterson good      30       5       0       0       0       0       0       0        15         0
s3 watterson poor      23       0       0       0       0      17       0       0        10         0
s4 awgn ramp up        93       0       0       0       0       0       0       0         7         0
s4 awgn ramp down      90       0       0       0       0       0       0       0        10         0
s4 watterson good      29       8       0       0       0       0       0       0        13         0
s4 watterson poor      21       0       0       0       0      17       0       0        12         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)

```

This run does not exactly reproduce the historical 0.914/0.762 record. Its
current robust seed-0 legacy ratios remain below both thresholds.
The robust seed-0 legacy bar fails exactly: `330144 <= 358848` and
`5 * 330144 < 4 * 429256`. Neither >1.0 versus best fixed nor >=0.8 versus oracle holds.

#### Standard control

| Seed | B delivered | B payload bits | Data air s | Switch air s | B x0 bps | B x1 bps | Joint best fixed bps | Joint oracle bps |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 | 273/300 | 351128 | 152.99 | 6.49 | 2295.1 | 2201.8 | 2491.4 | 3631.2 |
| 1 | 276/300 | 359616 | 156.81 | 6.49 | 2293.4 | 2202.3 | 2530.8 | 3663.6 |
| 2 | 270/300 | 351232 | 152.05 | 6.49 | 2310.0 | 2215.5 | 2498.2 | 3664.2 |
| 3 | 273/300 | 364472 | 146.43 | 6.49 | 2489.0 | 2383.4 | 2563.4 | 3667.8 |
| 4 | 273/300 | 363328 | 139.23 | 6.49 | 2609.5 | 2493.3 | 2574.3 | 3713.5 |

Legacy long-CP arm A control, delivered application bits per 300 frame slots.
These ratios are not comparable to the airtime ratios above.

| Seed | Adaptive bits | Best fixed bits | Oracle bits | a/bf | a/oracle |
|---|---:|---:|---:|---:|---:|
| 0 | 340968 | 371664 | 436712 | 0.917409273 | 0.780761692 |
| 1 | 349184 | 370240 | 436104 | 0.943128781 | 0.800689744 |
| 2 | 344040 | 364544 | 437400 | 0.943754389 | 0.786556927 |
| 3 | 348440 | 377360 | 438248 | 0.923362307 | 0.795074935 |
| 4 | 346376 | 370240 | 439312 | 0.935544512 | 0.788451033 |

Aggregate decision output, x0 then x1:

```text
arm B / best-fixed(joint) = 0.948 (x0) / 0.908 (x1)  (bar: > 1.0)
arm B / oracle(joint)     = 0.654 (x0) / 0.627 (x1)  (bar: >= 0.8)
```

Existing delay-spread segment output, retained as descriptive context only.
It does not measure fading-segment delivered-goodput non-regression.

```text
Grid = (fft_size/nc)/sample_rate = 0.41667 ms; DelayDomainEstimator clamps taps to 1..=8, so
the metric takes only EIGHT values, 0.000-2.9167 ms. CpGate::default_coppa() threshold is
2.5 ms and requires STRICTLY below, so buckets 0-5 are calm and 6-7 are not.
segment             0.000   0.417   0.833   1.250   1.667   2.083   2.500   2.917 undecoded  off_grid
s0 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s0 awgn ramp down      98       0       0       0       0       0       0       0         2         0
s0 watterson good      27      10       0       0       0       0       0       0        13         0
s0 watterson poor      23       0       0       0       0      15       0       0        12         0
s1 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s1 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s1 watterson good      23      14       0       0       0       0       0       0        13         0
s1 watterson poor      18       0       0       0       0      22       0       0        10         0
s2 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s2 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s2 watterson good      30       5       0       0       0       0       0       0        15         0
s2 watterson poor      14       0       0       0       0      22       0       0        14         0
s3 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s3 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s3 watterson good      32       4       0       0       0       0       0       0        14         0
s3 watterson poor      22       0       0       0       0      16       0       0        12         0
s4 awgn ramp up       100       0       0       0       0       0       0       0         0         0
s4 awgn ramp down      99       0       0       0       0       0       0       0         1         0
s4 watterson good      24      11       0       0       0       0       0       0        15         0
s4 watterson poor      17       0       0       0       0      22       0       0        11         0
off_grid total = 0 (MUST be 0; any nonzero value means the nc=48 / 8-tap
assumption is wrong and this diagnostic is invalid)

```

The standard aggregate x1 bar also fails. There is no supported claim of a
paired fading-tail gain, reduced volatility or new physical ceiling. COP-133
remains open on its intended outcome; the retained work is reconstruction,
measurement validation, receive-method equivalence and the negative applicability
record. The ignored grid took 253.76 seconds in this container; this experiment
runtime is not a production receive CPU benchmark.

