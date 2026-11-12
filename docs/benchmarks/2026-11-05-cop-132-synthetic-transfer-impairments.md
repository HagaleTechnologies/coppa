> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## Synthetic transfer impairments (COP-132)

Run the independent-frame diagnostic with:

```sh
cargo run -q -p coppa-bench --example impairment_transfer -- --level 2 --frames 2 --trials 1 --snr 30 --seed 12648430 --impairment all
```

Measured output on 2026-11-04, one trial with seed 12648430:

```csv
phy,level,channel,impairment,parameters,snr_db,frames,trials,seed,recovery_fraction,goodput_bps,airtime_s
v1-independent,2,awgn,none-v1,none,30.0,2,1,12648430,1.000000,685.71,2.730
v1-independent,2,awgn,impulsive-v1,p=0.001;amplitude_rms=10,30.0,2,1,12648430,1.000000,685.71,2.730
v1-independent,2,awgn,clipping-v1,threshold_rms=2,30.0,2,1,12648430,1.000000,685.71,2.730
v1-independent,2,awgn,agc-v1,fade_hz=1;fade_db=20;target_rms=0.5;block_samples=480;beta_attack=0.6;beta_release=0.85;gain_min=0.0001;gain_max=10000,30.0,2,1,12648430,1.000000,685.71,2.730
v1-independent,2,awgn,tilt-v1,low_hz=300;high_hz=2700;low_db=-6;high_db=6,30.0,2,1,12648430,1.000000,685.71,2.730

```

Defaults are level 2, two frames, ten trials, SNR 30 dB, seed 12648430,
channel `awgn` and impairment `all`. Levels 1 through 9 are accepted.
`--channel awgn|good|moderate|poor` selects AWGN or Watterson plus AWGN.
`--impairment all|none|impulsive|clipping|agc|tilt` selects all five rows in
that order, or one row. SNR is one finite signed value per invocation. Frames
and trials must be positive. Successful stdout contains only CSV; invalid
arguments fail before encoding and produce no CSV. Reports are collected before
stdout is written. No file is written into `results/`.

Preset labels are versioned. `none-v1` is the control. `impulsive-v1` adds
independent signed impulses with probability 0.001 per sample and magnitude ten
times clean RMS. `clipping-v1` symmetrically limits to twice clean RMS, bypassing
clipping for zero clean power. `agc-v1` stresses tracking with a 1 Hz, 20 dB
sinusoidal fade and fresh block AGC targeting 0.5 RMS every 480 samples, using
attack 0.6, release 0.85 and gain limits 0.0001 through 10000. `tilt-v1` shapes
300 through 2700 Hz from -6 to +6 dB, with unity at 1500 Hz and constant gains
outside that band. It is zero-phase finite-buffer FFT shaping with circular
boundaries, not a causal rig filter.

The encoder's frame buffers are concatenated. Clean power is measured first.
Tilt or the AGC stress fade precedes optional Watterson fading. AWGN follows,
referenced to the original clean power in 3000 Hz at 48000 samples/s. Impulses,
clipping or actual AGC follow noise. State continues across frame boundaries;
the buffer is then split at its original lengths. Effects are not normalized
away. Trials use seed plus trial index, with fixed XOR salts for Watterson
(0x3333333333333333), AWGN (0x5555555555555555) and impulses
(0x7777777777777777). Payloads and baseline channel draws are paired across cases.

Every frame uses a fresh receiver, including the control, so failed decodes
cannot carry HARQ state into unrelated frames, trials or cases. The legacy
`transfer_baseline` decoder policy is unchanged; its numbers are not directly
comparable as an impairment-only difference.

Recovery is the mean fraction of matching payload bytes, including accidental
matches in zero-filled failed frames. It is neither FER nor authenticated
delivery. `airtime_s` is the sum of encoded frame samples divided by 48000.
`goodput_bps` is sent payload bytes times eight times mean recovery divided by
that airtime. Airtime excludes ARQ, turnaround, CPU time and session overhead.
Failed transfers still have rows. These small synthetic runs impose no minimum
recovery threshold and make no confidence-bound or real-radio performance claim.
Physical rig/ALC behavior and group-delay ripple remain unmodeled.

