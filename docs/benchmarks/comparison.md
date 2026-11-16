# coppa same-simulator comparison (COP-170)

This page measures coppa through the channel grid of the 2020 Winlink
comparison: white Gaussian noise (WGN) and the Watterson Good (MPG) and Poor
(MPP) channels at 30, 25, 20, 15, 10, 5 and 0 dB SNR (3 kHz noise
bandwidth). One command regenerates it from the committed trial data:

```sh
cargo run --release -p coppa-bench -- compare
```

`cargo run --release -p coppa-bench -- compare --check` exits 0 when this page
matches `results/compare/` and 1 when it is stale.

## Scope

This page publishes coppa rows only. Runs of other modems stay with whoever
makes them and go to that project's maintainers first (COP-170 rescope,
2026-11-09). No other modem was run for any number here.

## What is measured

- **Stack.** Two library `Link`s from `coppa-engine` run in virtual time,
  joined by one `coppa-channel` `PersistentChannel` per direction, in 20 ms
  blocks with one block of pipeline delay. This is not coppad: the in-process
  `Link` and coppad were seen to disagree under fading during the COP-170
  research, so every row names its stack.
- **Row.** `link-l2`: level 2 at both ends, robust floor off, no rate loop.
- **Trial.** The caller dials at t = 0 and the answerer listens. The caller
  writes the payload when it reports CONNECTED, as a TNC host does. The
  payload is seeded random bytes. Outcomes: `complete` (every byte delivered),
  `drop` (either end left the session after connecting, before the last byte),
  `cap` (still running 600 s after the write for 2 KB, 1800 s for 16 KB) and
  `no_connect` (no CONNECTED within 120 s of the dial, or a failed handshake).
- **SNR reference.** The clean mean power of one 40-byte data frame at the
  row's level, recorded in every trial row as `ref_power`.
- **Seeds.** Trial t of cell c uses seed `0x17000000 + c * 0x10000 + t`, with
  `c = channel * 14 + snr_index * 2 + size_index` (WGN 0, MPG 1, MPP 2; 30 dB
  first; 2 KB first). A larger trial count extends the recorded trials.

## Metric definitions

- **Net bytes/min (per trial).** Payload bytes delivered to the answerer,
  divided by the minutes from the write to the arrival of the last byte. For a
  trial that dropped or hit the cap, the denominator runs to the end of the
  trial. A trial that never connected scores 0. Connect time is not in the
  denominator; the replay table reports its median.
- **Cell value.** The mean over the cell's n trials, failures included.
  **95% interval:** Student t on that mean with n - 1 degrees of freedom,
  clamped at 0. A zero-width interval means every trial gave the same number.
- **Completed/n.** Trials that delivered every byte; the replay table gives
  its Wilson 95% interval.
- **Source.** `results/compare/link-l2.csv`, one line per trial. Each cell
  links to its replay row, which gives the seed range and the exact command
  that re-runs the cell into a scratch directory.

## How this differs from the 2020 study

The channel names and the SNR grid are the same. The transfers are fixed
2 KB and 16 KB payloads instead of 5 to 10 minute timed runs with 4 to
120 KB files, and the stack runs in process instead of through audio hardware
and a hardware simulator.

## Status

Every table holds 8 trials per cell, 336 trials in all. With 8 trials, an
interval is wide wherever trials vary. The black-box self-test of coppad and
the Watterson cross-check against documented presets are below (COP-214).

## Tables

Each cell reads "mean (95% interval) completed/n" in net bytes/min.

<!-- compare:begin tables -->
### WGN (no fading), 2 KB transfer

| Row | 30 dB | 25 dB | 20 dB | 15 dB | 10 dB | 5 dB | 0 dB |
|---|---|---|---|---|---|---|---|
| coppa 0.16.0, library Link, level 2 (BPSK 1/2), floor off | [1494 (1494-1494) 8/8](#cell-link-l2-wgn-30-2048) | [1494 (1494-1494) 8/8](#cell-link-l2-wgn-25-2048) | [1494 (1494-1494) 8/8](#cell-link-l2-wgn-20-2048) | [1494 (1494-1494) 8/8](#cell-link-l2-wgn-15-2048) | [1494 (1494-1494) 8/8](#cell-link-l2-wgn-10-2048) | [1473 (1438-1508) 8/8](#cell-link-l2-wgn-5-2048) | [0 (0-0) 0/8](#cell-link-l2-wgn-0-2048) |

### WGN (no fading), 16 KB transfer

| Row | 30 dB | 25 dB | 20 dB | 15 dB | 10 dB | 5 dB | 0 dB |
|---|---|---|---|---|---|---|---|
| coppa 0.16.0, library Link, level 2 (BPSK 1/2), floor off | [1512 (1512-1512) 8/8](#cell-link-l2-wgn-30-16384) | [1512 (1512-1512) 8/8](#cell-link-l2-wgn-25-16384) | [1512 (1512-1512) 8/8](#cell-link-l2-wgn-20-16384) | [1512 (1512-1512) 8/8](#cell-link-l2-wgn-15-16384) | [1512 (1512-1512) 8/8](#cell-link-l2-wgn-10-16384) | [1503 (1490-1516) 8/8](#cell-link-l2-wgn-5-16384) | [0 (0-0) 0/8](#cell-link-l2-wgn-0-16384) |

### MPG (Watterson Good: 0.5 ms, 0.1 Hz), 2 KB transfer

| Row | 30 dB | 25 dB | 20 dB | 15 dB | 10 dB | 5 dB | 0 dB |
|---|---|---|---|---|---|---|---|
| coppa 0.16.0, library Link, level 2 (BPSK 1/2), floor off | [1193 (1098-1288) 8/8](#cell-link-l2-mpg-30-2048) | [1200 (997-1402) 8/8](#cell-link-l2-mpg-25-2048) | [1213 (1094-1332) 8/8](#cell-link-l2-mpg-20-2048) | [1127 (971-1283) 8/8](#cell-link-l2-mpg-15-2048) | [704 (480-928) 8/8](#cell-link-l2-mpg-10-2048) | [38 (14-62) 0/8](#cell-link-l2-mpg-5-2048) | [0 (0-0) 0/8](#cell-link-l2-mpg-0-2048) |

### MPG (Watterson Good: 0.5 ms, 0.1 Hz), 16 KB transfer

| Row | 30 dB | 25 dB | 20 dB | 15 dB | 10 dB | 5 dB | 0 dB |
|---|---|---|---|---|---|---|---|
| coppa 0.16.0, library Link, level 2 (BPSK 1/2), floor off | [1234 (1160-1309) 8/8](#cell-link-l2-mpg-30-16384) | [1251 (1162-1340) 8/8](#cell-link-l2-mpg-25-16384) | [1256 (1184-1328) 8/8](#cell-link-l2-mpg-20-16384) | [1105 (1049-1162) 8/8](#cell-link-l2-mpg-15-16384) | [580 (517-644) 5/8](#cell-link-l2-mpg-10-16384) | [51 (25-76) 0/8](#cell-link-l2-mpg-5-16384) | [0 (0-0) 0/8](#cell-link-l2-mpg-0-16384) |

### MPP (Watterson Poor: 2 ms, 1 Hz), 2 KB transfer

| Row | 30 dB | 25 dB | 20 dB | 15 dB | 10 dB | 5 dB | 0 dB |
|---|---|---|---|---|---|---|---|
| coppa 0.16.0, library Link, level 2 (BPSK 1/2), floor off | [386 (192-579) 7/8](#cell-link-l2-mpp-30-2048) | [219 (64-374) 3/8](#cell-link-l2-mpp-25-2048) | [126 (41-210) 1/8](#cell-link-l2-mpp-20-2048) | [129 (57-200) 1/8](#cell-link-l2-mpp-15-2048) | [48 (12-83) 0/8](#cell-link-l2-mpp-10-2048) | [0 (0-0) 0/8](#cell-link-l2-mpp-5-2048) | [0 (0-0) 0/8](#cell-link-l2-mpp-0-2048) |

### MPP (Watterson Poor: 2 ms, 1 Hz), 16 KB transfer

| Row | 30 dB | 25 dB | 20 dB | 15 dB | 10 dB | 5 dB | 0 dB |
|---|---|---|---|---|---|---|---|
| coppa 0.16.0, library Link, level 2 (BPSK 1/2), floor off | [240 (152-328) 0/8](#cell-link-l2-mpp-30-16384) | [205 (116-294) 0/8](#cell-link-l2-mpp-25-16384) | [134 (41-228) 0/8](#cell-link-l2-mpp-20-16384) | [107 (77-137) 0/8](#cell-link-l2-mpp-15-16384) | [19 (1-36) 0/8](#cell-link-l2-mpp-10-16384) | [5 (0-17) 0/8](#cell-link-l2-mpp-5-16384) | [0 (0-0) 0/8](#cell-link-l2-mpp-0-16384) |
<!-- compare:end tables -->

## Replay

<!-- compare:begin replay -->
| Cell | n | completed (Wilson 95%) | drops | no connect | median connect | seeds | replay |
|---|---|---|---|---|---|---|---|
| <a id="cell-link-l2-wgn-30-2048"></a>link-l2, WGN, 30 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17000000-0x17000007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 30 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-30-16384"></a>link-l2, WGN, 30 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17010000-0x17010007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 30 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-25-2048"></a>link-l2, WGN, 25 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17020000-0x17020007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 25 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-25-16384"></a>link-l2, WGN, 25 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17030000-0x17030007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 25 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-20-2048"></a>link-l2, WGN, 20 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17040000-0x17040007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 20 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-20-16384"></a>link-l2, WGN, 20 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17050000-0x17050007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 20 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-15-2048"></a>link-l2, WGN, 15 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17060000-0x17060007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 15 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-15-16384"></a>link-l2, WGN, 15 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17070000-0x17070007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 15 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-10-2048"></a>link-l2, WGN, 10 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17080000-0x17080007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 10 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-10-16384"></a>link-l2, WGN, 10 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x17090000-0x17090007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 10 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-5-2048"></a>link-l2, WGN, 5 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x170a0000-0x170a0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 5 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-5-16384"></a>link-l2, WGN, 5 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.26 s | 0x170b0000-0x170b0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 5 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-0-2048"></a>link-l2, WGN, 0 dB, 2 KB | 8 | 0/8 (0.00-0.32) | 0 | 8 | - | 0x170c0000-0x170c0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 0 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-wgn-0-16384"></a>link-l2, WGN, 0 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 0 | 8 | - | 0x170d0000-0x170d0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel wgn --snr 0 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-30-2048"></a>link-l2, MPG, 30 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x170e0000-0x170e0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 30 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-30-16384"></a>link-l2, MPG, 30 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x170f0000-0x170f0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 30 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-25-2048"></a>link-l2, MPG, 25 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x17100000-0x17100007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 25 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-25-16384"></a>link-l2, MPG, 25 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x17110000-0x17110007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 25 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-20-2048"></a>link-l2, MPG, 20 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x17120000-0x17120007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 20 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-20-16384"></a>link-l2, MPG, 20 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x17130000-0x17130007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 20 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-15-2048"></a>link-l2, MPG, 15 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x17140000-0x17140007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 15 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-15-16384"></a>link-l2, MPG, 15 dB, 16 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x17150000-0x17150007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 15 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-10-2048"></a>link-l2, MPG, 10 dB, 2 KB | 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s | 0x17160000-0x17160007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 10 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-10-16384"></a>link-l2, MPG, 10 dB, 16 KB | 8 | 5/8 (0.31-0.86) | 0 | 0 | 3.30 s | 0x17170000-0x17170007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 10 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-5-2048"></a>link-l2, MPG, 5 dB, 2 KB | 8 | 0/8 (0.00-0.32) | 6 | 1 | 8.30 s | 0x17180000-0x17180007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 5 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-5-16384"></a>link-l2, MPG, 5 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 8 | 0 | 8.30 s | 0x17190000-0x17190007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 5 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-0-2048"></a>link-l2, MPG, 0 dB, 2 KB | 8 | 0/8 (0.00-0.32) | 0 | 8 | - | 0x171a0000-0x171a0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 0 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpg-0-16384"></a>link-l2, MPG, 0 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 0 | 8 | - | 0x171b0000-0x171b0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 0 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-30-2048"></a>link-l2, MPP, 30 dB, 2 KB | 8 | 7/8 (0.53-0.98) | 1 | 0 | 5.80 s | 0x171c0000-0x171c0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 30 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-30-16384"></a>link-l2, MPP, 30 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 4 | 0 | 3.30 s | 0x171d0000-0x171d0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 30 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-25-2048"></a>link-l2, MPP, 25 dB, 2 KB | 8 | 3/8 (0.14-0.69) | 3 | 0 | 3.30 s | 0x171e0000-0x171e0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 25 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-25-16384"></a>link-l2, MPP, 25 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 6 | 0 | 3.30 s | 0x171f0000-0x171f0007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 25 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-20-2048"></a>link-l2, MPP, 20 dB, 2 KB | 8 | 1/8 (0.02-0.47) | 2 | 1 | 3.30 s | 0x17200000-0x17200007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 20 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-20-16384"></a>link-l2, MPP, 20 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 6 | 1 | 3.30 s | 0x17210000-0x17210007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 20 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-15-2048"></a>link-l2, MPP, 15 dB, 2 KB | 8 | 1/8 (0.02-0.47) | 3 | 0 | 5.80 s | 0x17220000-0x17220007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 15 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-15-16384"></a>link-l2, MPP, 15 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 6 | 0 | 5.80 s | 0x17230000-0x17230007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 15 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-10-2048"></a>link-l2, MPP, 10 dB, 2 KB | 8 | 0/8 (0.00-0.32) | 6 | 1 | 8.30 s | 0x17240000-0x17240007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 10 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-10-16384"></a>link-l2, MPP, 10 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 6 | 2 | 5.80 s | 0x17250000-0x17250007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 10 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-5-2048"></a>link-l2, MPP, 5 dB, 2 KB | 8 | 0/8 (0.00-0.32) | 2 | 6 | 5.80 s | 0x17260000-0x17260007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 5 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-5-16384"></a>link-l2, MPP, 5 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 1 | 7 | 13.30 s | 0x17270000-0x17270007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 5 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-0-2048"></a>link-l2, MPP, 0 dB, 2 KB | 8 | 0/8 (0.00-0.32) | 0 | 8 | - | 0x17280000-0x17280007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 0 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
| <a id="cell-link-l2-mpp-0-16384"></a>link-l2, MPP, 0 dB, 16 KB | 8 | 0/8 (0.00-0.32) | 0 | 8 | - | 0x17290000-0x17290007 | `cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpp --snr 0 --size 16384 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay` |
<!-- compare:end replay -->

## Black-box self-test

The daemon self-test uses two unmodified `coppad` processes. These results
test the audio cable and host interface; they are separate from the library
`Link` rows above. Real-time scheduling means fixed seeds do not make these
trials bit-reproducible. Connect time runs from dial to the caller's CONNECTED.
Wall time runs from dial to trial end. Cable lead is measured by the integrity
probe. Starved and overflow ticks count 20 ms relay intervals during the trial.
Source: `results/compare/blackbox-coppad.csv`.

<!-- compare:begin blackbox-selftest -->
| Modem | Cell | n | completed | net B/min, mean (95% interval) | median connect | median wall | drops | cable lead | starved / overflow ticks |
|---|---|---|---|---|---|---|---|---|---|
| coppad 0.16.0, VARA-style host, HF_STANDARD | WGN, 30 dB, 2 KB | 3 | 3/3 | 1398 (0-3527) | 7.1 s | 143.2 s | 0 | 355 ms | 0 / 0 |
| coppad 0.16.0, VARA-style host, HF_STANDARD | MPG, 20 dB, 2 KB | 3 | 3/3 | 227 (183-271) | 7.1 s | 548.3 s | 0 | 355 ms | 0 / 0 |
<!-- compare:end blackbox-selftest -->

## Channel cross-check

The Watterson presets use the MPG, MPM and MPP settings from the 2020 study.
codec2's `README_ofdm.md` describes its CCIR Poor preset as 1 Hz Doppler and
1 ms delay. Its delay is half of coppa's MPP 2 ms. PathSim defines no matching
delay presets, so that offset is not measured.

<!-- compare:begin channel-check -->
| Preset | Delay set / measured | Path power ratio | Doppler spread (2 sigma) set / measured | 3 kHz noise reference error | Within tolerance |
|---|---|---|---|---|---|
| Good (MPG) | 0.500 / 0.500 ms | -0.26 dB | 0.10 / 0.10 Hz | +0.01 dB | yes |
| Moderate | 1.000 / 1.000 ms | +0.06 dB | 0.50 / 0.52 Hz | -0.00 dB | yes |
| Poor (MPP) | 2.000 / 2.000 ms | +0.62 dB | 1.00 / 0.98 Hz | -0.01 dB | yes |
<!-- compare:end channel-check -->

## Provenance and clean-room statement

Built from this repository's own code and documents and from the published
channel grid of the 2020 Winlink comparison. No other modem's source was read
and no other modem was run for any number on this page. Recording platform,
toolchain and commands are in `results/compare/metadata.md`.

The host dialect definitions come from `docs/HOST-API.md` (VARA-style and
KISS) and `docs/ARDOP.md` (ARDOP). KISS byte framing follows the
[KISS TNC paper](https://www.ka9q.net/papers/kiss.html). Channel preset
parameters come from the [IONOS manual](https://winlink.org/sites/default/files/downloads/ionos_sim_manual.pdf)
and the [2020 comparison study](https://winlink.org/sites/default/files/downloads/a_winlink_digital_mode_performance_comparison_based_on_the_ionis_sim_hf_vhf_channel_simulator_-_november_2_2020_0.pdf).
The documented offset uses codec2's
[README_ofdm.md](https://github.com/drowe67/codec2/blob/main/README_ofdm.md).
[PathSim's page](https://www.moetronix.com/ae4jy/pathsim.htm) has no
corresponding preset delay table; no PathSim run was made.
