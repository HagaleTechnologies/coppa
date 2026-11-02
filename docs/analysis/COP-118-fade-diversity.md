# COP-118: which fade-diversity mechanism keeps a coppa link up

Date: 2026-11-02. Ticket: COP-118, the first slice of COP-66. Base commit: `a6b14ef`.
Status: measurement report. This is simulation evidence. Both prototypes are throwaway code and
nothing in the product crates changed.

## Recommendation

Build the concatenated long outer code. The variant measured here is a systematic RS(12,6) erasure
code over GF(256), applied across six NR-coded level-2 packets and sent as twelve NR-coded packets
per ARQ segment. The receiver rebuilds the segment from any six packets that pass CRC.

On the adapted CCIR-Moderate session bench it completed **5/5** ten-minute sessions at a mean
**1579.3 bytes/min**. The matched control completed 1/5 at 709.4 bytes/min. Frequency repetition
completed 2/5 at 465.9 bytes/min. On Poor the outer code also completed 5/5, at 1107.0 bytes/min.
Frequency repetition completed 3/5 there and the matched control 0/5.

The price of the outer code:

- On a clean channel it delivers 45.4% less than the matched control (2254.4 vs 4127.9
  bytes/min), so each delivered byte costs 1.83x the control's airtime.
- A segment takes 16.4 s on air, twice the control's 8.2 s, so 16.4 s is also the minimum
  delivery latency. On Moderate the median block latency was 16.4 s and the worst was 267.1 s.

None of the five completed Moderate outer-code sessions had a block waiting for a retry at the
600 s cutoff, and on Poor two had one block on its first send. So the 5/5 is not a block caught
mid-retry by the cutoff.

The ten-minute target is met here only in simulation: five deterministic trials per preset, ideal
ACK reception, and the adapted harness described below. A 5/5 result is a feasibility signal for
building this mechanism. It is not a reliability guarantee, and no RF trial was run.

Frequency repetition is not recommended. On Moderate it kept 2/5 sessions up and cost 34.3%
throughput against the matched control. It protects only the payload. The preamble and header are
sent as before, and on Moderate sync or header failures took 22-30% of its decoded packets, more
than its 14-18% payload failures. On Good its payload failures all but vanished, yet one session
still dropped on sync and header losses. The result settles this variant only, not every
frequency-diversity design. The same caveat applies to the outer code: it is one long-codeword
design, not an enlarged NR LDPC graph.

## What was compared

Two experiments, kept apart:

1. **Original control.** `cargo run --release -p coppa-bench --example session`, unchanged. It
   reproduces the ticket's 0/5 Moderate and 0/5 Poor result.
2. **Adapted comparison.** A scratch crate (source in the appendix) that runs three arms on one
   harness:
   - `matched-control`: a logical block of six level-2 packets. The block is complete only when all
     six decode.
   - `freq-rep2`: the same six packets. Coded symbol `k` of 1944 goes into OFDM symbol `k / 22`,
     on data slot `k % 22` and again on data slot `k % 22 + 22` of that symbol's 44 data slots.
     `insert_pilots` places the 4 pilots, so the pair is 22 to 26 carriers apart, 1100-1300 Hz,
     nominally 1200 Hz: half the 48-carrier, 50 Hz-spaced active band. The payload takes 89 OFDM
     symbols instead of 45. In the last one only slots 0-7 and 22-29 carry data, and the rest are
     padded with +1 as `modulate_mapped` pads. The header says `codewords: 2`, so the unchanged
     demodulator returns both copies. The receiver combines each pair by maximum-ratio combining
     with its own per-carrier noise estimates, before NR decoding. For BPSK this equals adding the
     two LLRs. Power: `modulate_mapped` clips every arm's frame to the same PAPR target and scales it to
     the same peak, so each carrier carries about the same power as in the control. The repeated
     symbols cost airtime, not extra power.
   - `outer-rs12-6`: the six chunks plus six RS parity chunks, sent as twelve ordinary NR-protected
     packets. The block is complete from any six CRC-valid packets.

The cross-frame interleaver that `BENCHMARKS.md` ("Transfer-level cross-frame interleaving")
already rejected was not repeated. The outer code adds new parity across packets. It does not
stripe independent codewords.

## Harness

Unchanged from `crates/coppa-bench/examples/session.rs`, in all three arms:

- Speed level 2 (BPSK, NR BG2 rate 1/2) and ARQ window 8.
- The real `ArqTx`/`ArqRx` state machines with a five-retransmit budget. A session drops when
  `ArqTx::is_failed` becomes true.
- A 600 s session with a 20 -> 0 -> 20 dB SNR ramp, 150 ms turnaround, and ideal ACKs that are
  charged one level-2 frame of airtime plus two turnarounds.
- Seeds `0x5E55_1000 ^ (preset << 16) ^ trial`, as in `session.rs`.

Changed for the comparison, in all three arms alike:

- **Channel.** Each trial has one continuous Watterson realization. Both tap-gain traces come from
  `coppa_channel::watterson::watterson_with_gains`, run once at 100 Hz over 900 s, with seed
  `seed ^ 0x3333_3333_3333_3333`. The complex gains are linearly interpolated onto the absolute
  48 kHz time of every transmitted sample. The preset's tap delays (0.5, 1 or 2 ms) are applied to
  the analytic signal. The trace advances through data, retries, ACK time and idle gaps, and is
  never renormalized per frame. At each frame's start the delayed tap contributes nothing for its
  first 24, 48 or 96 samples, as in `watterson()`, because nothing was sent just before. A trial
  that would read past the end of its 900 s trace stops with an error, and none did. The original
  bench instead draws an independent realization per frame.
- **Noise.** The SNR is the ramp value at each packet's midpoint. The noise uses
  `awgn_ref_seeded`, the 3 kHz convention, with one fixed power reference (the mean power of one
  clean control packet, 2.2229e-2). The noise seed is `seed ^ 0x5555_5555_5555_5555 ^
  start_sample`, so arms share an absolute-time noise schedule.
- **Segment.** One ARQ segment is one 678-byte logical block: six 113-byte chunks, each with 4
  bytes of prototype metadata (block id, packet index, packet count). That fills the 117-byte
  level-2 payload. The packets of a block go back to back, and the block is ACKed once, after the
  whole block. A retry resends the whole block. The receiver keeps CRC-valid packets from earlier
  attempts of the same block id. Nothing else carries over between attempts. The original bench
  ACKs every frame.
- **RTO.** The initial RTO uses the `arq::rto_floor` formula with the arm's block airtime in place
  of one frame's: `8 x block + 2 x 0.15 s + ACK`. That is capped at the ARQ's 60 s backoff ceiling,
  and the cap applies to all three arms. The floor inside `ArqTx` is still the frame-based one.
  Backoff and retry budget are unchanged.
- **Receiver.** Every arm uses the same chain: the HF 601-tap RX bandpass, then
  `CoppaModem::demodulate_frame`, then soft demap, deinterleave, rate dematch at RV0, pad pinning,
  NR BG2 decode and CRC-32. This copies `CoppaTransceiver::receive_single_codeword` without turbo
  re-estimation and without the IR-HARQ accumulator. The receiver also listens for 1200 samples
  after each frame, which is not charged as airtime (see "Measured limits").

Failure counts: "sync/header" means `demodulate_frame` returned nothing. "Payload" means the NR
decoder did not converge or the CRC failed. No decoded header disagreed with the arm, and no
rebuilt block differed from the sent block. A retry still sends packets the receiver already holds,
and their airtime is charged, but the receiver does not decode them again. Loss rates below
therefore divide by packets actually decoded (`decodes`), not by packets sent.

## Prototype checks

All passed before the measurement, in the same run:

```text
CHECK clean exact-byte recovery: PASS (3 packets x 3 arms)
CHECK replica alignment: PASS (1944 coded symbols, 3916 demodulated symbols)
CHECK combining with one replica erased: PASS
CHECK RS(12,6): PASS (924 six-packet subsets exact, five-packet sets refused)
CHECK identity channel: PASS (max abs error 4.47e-7)
CHECK bare-frame symbols: 1936 bare vs 1980 with a 1200-sample tail (1944 needed)
PROFILE fft 960 cp 300 spacing 50.000 Hz, active carriers 48, data/OFDM symbol 44, replica separation ~1200 Hz (half the active band)
ARM matched-control: 6 packets/block, packet 1.365 s, block 8.190 s, block bytes 678, initial RTO 60.0 s
ARM freq-rep2: 6 packets/block, packet 2.520 s, block 15.120 s, block bytes 678, initial RTO 60.0 s
ARM outer-rs12-6: 12 packets/block, packet 1.365 s, block 16.380 s, block bytes 678, initial RTO 60.0 s
PRESET good: delay 0.5 ms, sigma 0.05 Hz, |rho_f(sep)| = 0.31, rho_t(1 s) 0.952, rho_t(5 s) 0.291
TRACE good: mean tap power 0.958 (expect 1.000); autocorrelation | 0.5s 0.987 vs 0.988 | 1s 0.951 vs 0.952 | 2s 0.816 vs 0.821 | 5s 0.278 vs 0.291
PRESET moderate: delay 1.0 ms, sigma 0.25 Hz, |rho_f(sep)| = 0.81, rho_t(1 s) 0.291, rho_t(5 s) 0.000
TRACE moderate: mean tap power 0.982 (expect 1.000); autocorrelation | 0.5s 0.730 vs 0.735 | 1s 0.280 vs 0.291 | 2s 0.003 vs 0.007 | 5s -0.004 vs 0.000
PRESET poor: delay 2.0 ms, sigma 0.50 Hz, |rho_f(sep)| = 0.31, rho_t(1 s) 0.007, rho_t(5 s) 0.000
TRACE poor: mean tap power 0.992 (expect 1.000); autocorrelation | 0.5s 0.284 vs 0.291 | 1s -0.002 vs 0.007 | 2s -0.001 vs 0.000 | 5s -0.002 vs 0.000
PREF 2.222894e-2
POWER freq-rep2: mean packet power 1.725312e-2 (-1.10 dB vs control)
POWER outer-rs12-6: mean packet power 2.222894e-2 (+0.00 dB vs control)
```

The `TRACE` lines average 20 traces over 600 s each. Ensemble tap power is within 4.2% of 1, and
the measured autocorrelation follows the generator's Gaussian-Doppler form
`exp(-2 pi^2 sigma^2 tau^2)`. Linear interpolation between 10 ms points is far finer than any
preset's fade rate.

**Does the outer code span several fades?** Yes. The temporal correlation of the channel across
one 16.38 s outer-code block is 0.000 on Moderate and Poor, and 0.000 on Good, to three places.
Across one 1.365 s packet it is 0.10 on Moderate, 0.912 on Good and 0.000 on Poor. One Moderate
block therefore covers many independent fades. One Good block covers about three, since Good's
correlation is 0.29 at 5 s.

## Results

### Original bench (unchanged `session.rs`, production receiver with turbo and HARQ)

| Experiment | Preset | Mechanism | Survival | Mean bytes/min | Throughput cost |
|---|---|---|---|---|---|
| Original session bench | Good | Baseline | 2/5 | 1314.3 | 0% |
| Original session bench | Moderate | Baseline | 0/5 | 1057.8 | 0% |
| Original session bench | Poor | Baseline | 0/5 | 342.1 | 0% |

Per trial, as elapsed seconds, delivered bytes and bytes/min:

| Preset | Trial 0 | Trial 1 | Trial 2 | Trial 3 | Trial 4 |
|---|---|---|---|---|---|
| Good | completed 602.7 s, 15912, 1584.1 | DROPPED 491.9 s, 9126, 1113.1 | completed 600.0 s, 15795, 1579.4 | DROPPED 449.5 s, 8892, 1186.9 | DROPPED 449.9 s, 8307, 1107.9 |
| Moderate | DROPPED 399.3 s, 6903, 1037.2 | DROPPED 403.8 s, 7137, 1060.6 | DROPPED 371.5 s, 6669, 1077.0 | DROPPED 408.8 s, 7137, 1047.6 | DROPPED 368.5 s, 6552, 1066.8 |
| Poor | DROPPED 293.3 s, 1755, 359.0 | DROPPED 266.4 s, 1638, 368.9 | DROPPED 246.4 s, 1521, 370.4 | DROPPED 250.5 s, 1404, 336.3 | DROPPED 279.8 s, 1287, 276.0 |

### Adapted comparison (same harness for all arms)

Survival is sessions that reached 600 s without `ArqTx::is_failed`. Mean bytes/min averages all
five trials, dropped ones included, each as delivered bytes over its own elapsed minutes. The
throughput cost is `100 x (1 - candidate / matched-control)` within the same preset, so a negative
cost is a gain. Latency runs from block submission to in-order delivery and pools every delivered
block across the five trials.

| Preset | Mechanism | Survival | Mean bytes/min | Throughput cost | Median latency | Max latency |
|---|---|---|---|---|---|---|
| Good | matched-control | 5/5 | 2360.7 | 0% | 26.2 s | 279.7 s |
| Good | freq-rep2 | 4/5 | 1333.6 | 43.5% | 31.9 s | 294.4 s |
| Good | outer-rs12-6 | 5/5 | 1924.2 | 18.5% | 16.4 s | 150.8 s |
| **Moderate** | matched-control | 1/5 | 709.4 | 0% | 190.9 s | 297.1 s |
| **Moderate** | freq-rep2 | 2/5 | 465.9 | 34.3% | 97.4 s | 433.4 s |
| **Moderate** | outer-rs12-6 | **5/5** | **1579.3** | **-122.6%** | 16.4 s | 267.1 s |
| Poor | matched-control | 0/5 | 115.0 | 0% | 139.9 s | 148.1 s |
| Poor | freq-rep2 | 3/5 | 440.3 | -283.0% | 246.9 s | 415.9 s |
| Poor | outer-rs12-6 | 5/5 | 1107.0 | -862.9% | 101.6 s | 388.4 s |

Clean channel, no fading and no noise, one 600 s session per arm. These rows separate coding and
airtime overhead from loss:

| Mechanism | Bytes/min | Throughput cost | Seconds per delivered byte | Airtime cost vs control |
|---|---|---|---|---|
| matched-control | 4127.9 | 0% | 601.2 s / 41358 B = 14.54 ms | 1.00x |
| freq-rep2 | 2423.6 | 41.3% | 604.3 s / 24408 B = 24.76 ms | 1.70x |
| outer-rs12-6 | 2254.4 | 45.4% | 613.5 s / 23052 B = 26.61 ms | 1.83x |

The adapted matched control is not the original bench. It completes 5/5 Good sessions where the
original completes 2/5. The two harnesses differ in segment size, per-block ACK, packet retention
across attempts, the 60 s initial RTO, the continuous channel, and the receiver (no turbo, no
HARQ). Compare arms only within one table.

### Per-trial records (adapted comparison)

`air` is data airtime, ACK airtime and turnaround time in seconds. `tx` counts block transmissions
and physical packets, and `dec` counts packets decoded. `fail` counts lost packets as sync/header
and payload. `pending` is the number of blocks still unacknowledged when the session ended and the
highest send count among them; a block fails on its sixth send, so a dropped session shows 6.
`RTO` is the ARQ's retransmission timeout at the end. Every session overshoots 600 s by at most one
ARQ step, as in `session.rs`, and the overshoot is in `elapsed`.

| Mechanism | Preset | Trial | Seed | Status | Elapsed s | Bytes | Bytes/min | Blocks | tx (blocks/packets) | dec | air (data/ACK/turn) | Median / max latency s | fail (sync-hdr/payload) | pending (blocks/max sends) | RTO s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| matched-control | clean | 0 | `0x5e551000` | completed | 601.2 | 41358 | 4127.9 | 61 | 61/366 | 366 | 499.6/83.3/18.3 | 8.2 / 8.2 | 0/0 | 0/0 | 12.6 |
| matched-control | good | 0 | `0x5e551000` | completed | 607.9 | 23730 | 2342.0 | 35 | 64/384 | 279 | 524.2/47.8/10.5 | 26.2 / 279.7 | 59/10 | 0/0 | 25.2 |
| matched-control | good | 1 | `0x5e551001` | completed | 617.8 | 25086 | 2436.2 | 37 | 67/402 | 315 | 548.7/50.5/11.1 | 26.2 / 238.6 | 80/13 | 0/0 | 25.2 |
| matched-control | good | 2 | `0x5e551002` | completed | 607.3 | 23730 | 2344.4 | 35 | 66/396 | 282 | 540.5/47.8/10.5 | 26.2 / 265.9 | 55/17 | 0/0 | 12.6 |
| matched-control | good | 3 | `0x5e551003` | completed | 608.1 | 23052 | 2274.3 | 34 | 66/396 | 271 | 540.5/46.4/10.2 | 36.1 / 194.6 | 56/11 | 0/0 | 12.6 |
| matched-control | good | 4 | `0x5e551004` | completed | 608.5 | 24408 | 2406.8 | 36 | 66/396 | 299 | 540.5/49.1/10.8 | 24.6 / 249.0 | 66/17 | 0/0 | 25.2 |
| matched-control | moderate | 0 | `0x5e541000` | DROPPED | 338.6 | 2034 | 360.4 | 3 | 35/210 | 104 | 286.7/8.2/1.8 | 129.3 / 131.0 | 14/29 | 5/6 | 60.0 |
| matched-control | moderate | 1 | `0x5e541001` | DROPPED | 596.3 | 8136 | 818.7 | 12 | 63/378 | 245 | 516.0/23.2/5.1 | 192.6 / 287.3 | 67/64 | 3/6 | 60.0 |
| matched-control | moderate | 2 | `0x5e541002` | completed | 613.0 | 11526 | 1128.1 | 17 | 67/402 | 243 | 548.7/23.2/5.1 | 256.2 / 297.1 | 61/67 | 3/3 | 60.0 |
| matched-control | moderate | 3 | `0x5e541003` | DROPPED | 454.5 | 4746 | 626.5 | 7 | 52/312 | 154 | 425.9/13.7/3.0 | 139.2 / 281.1 | 32/42 | 5/6 | 60.0 |
| matched-control | moderate | 4 | `0x5e541004` | DROPPED | 397.9 | 4068 | 613.4 | 6 | 46/276 | 150 | 376.7/9.6/2.1 | 85.2 / 154.6 | 38/40 | 7/6 | 60.0 |
| matched-control | poor | 0 | `0x5e571000` | DROPPED | 348.5 | 678 | 116.7 | 1 | 41/246 | 122 | 335.8/1.4/0.3 | 142.7 / 142.7 | 32/51 | 8/6 | 60.0 |
| matched-control | poor | 1 | `0x5e571001` | DROPPED | 312.0 | 0 | 0.0 | 0 | 28/168 | 89 | 229.3/5.5/1.2 | n/a | 15/31 | 4/6 | 60.0 |
| matched-control | poor | 2 | `0x5e571002` | DROPPED | 355.2 | 2712 | 458.1 | 4 | 38/228 | 133 | 311.2/8.2/1.8 | 139.9 / 148.1 | 28/44 | 6/6 | 60.0 |
| matched-control | poor | 3 | `0x5e571003` | DROPPED | 297.4 | 0 | 0.0 | 0 | 26/156 | 87 | 212.9/8.2/1.8 | n/a | 11/30 | 2/6 | 60.0 |
| matched-control | poor | 4 | `0x5e571004` | DROPPED | 311.7 | 0 | 0.0 | 0 | 29/174 | 83 | 237.5/5.5/1.2 | n/a | 18/23 | 4/6 | 60.0 |
| freq-rep2 | clean | 0 | `0x5e551000` | completed | 604.3 | 24408 | 2423.6 | 36 | 36/216 | 216 | 544.3/49.1/10.8 | 15.1 / 15.1 | 0/0 | 0/0 | 16.8 |
| freq-rep2 | good | 0 | `0x5e551000` | completed | 612.9 | 15594 | 1526.7 | 23 | 38/228 | 174 | 574.6/31.4/6.9 | 31.9 / 283.8 | 36/0 | 0/0 | 17.2 |
| freq-rep2 | good | 1 | `0x5e551001` | completed | 614.5 | 16272 | 1588.8 | 24 | 38/228 | 190 | 574.6/32.8/7.2 | 15.1 / 233.5 | 46/0 | 0/0 | 17.2 |
| freq-rep2 | good | 2 | `0x5e551002` | completed | 609.5 | 14238 | 1401.6 | 21 | 38/228 | 164 | 574.6/28.7/6.3 | 62.1 / 250.2 | 37/1 | 0/0 | 18.2 |
| freq-rep2 | good | 3 | `0x5e551003` | completed | 609.5 | 14238 | 1401.6 | 21 | 38/228 | 159 | 574.6/28.7/6.3 | 78.9 / 231.8 | 33/0 | 0/0 | 18.2 |
| freq-rep2 | good | 4 | `0x5e551004` | DROPPED | 542.9 | 6780 | 749.3 | 10 | 34/204 | 136 | 514.1/23.2/5.1 | 15.1 / 294.4 | 29/0 | 1/6 | 40.3 |
| freq-rep2 | moderate | 0 | `0x5e541000` | completed | 643.2 | 7458 | 695.7 | 11 | 41/246 | 150 | 619.9/19.1/4.2 | 97.4 / 433.4 | 33/24 | 2/3 | 60.0 |
| freq-rep2 | moderate | 1 | `0x5e541001` | completed | 623.1 | 7458 | 718.1 | 11 | 40/240 | 150 | 604.8/15.0/3.3 | 245.2 / 339.3 | 33/26 | 5/3 | 60.0 |
| freq-rep2 | moderate | 2 | `0x5e541002` | DROPPED | 495.3 | 2034 | 246.4 | 3 | 32/192 | 115 | 483.8/8.2/1.8 | 77.3 / 77.3 | 34/21 | 5/6 | 60.0 |
| freq-rep2 | moderate | 3 | `0x5e541003` | DROPPED | 493.8 | 2712 | 329.5 | 4 | 32/192 | 114 | 483.8/8.2/1.8 | 94.0 / 110.8 | 32/18 | 6/6 | 60.0 |
| freq-rep2 | moderate | 4 | `0x5e541004` | DROPPED | 479.2 | 2712 | 339.6 | 4 | 31/186 | 112 | 468.7/8.2/1.8 | 142.7 / 157.9 | 33/16 | 6/6 | 60.0 |
| freq-rep2 | poor | 0 | `0x5e571000` | completed | 622.5 | 4746 | 457.5 | 7 | 40/240 | 129 | 604.8/13.7/3.0 | 385.7 / 415.9 | 33/27 | 2/4 | 60.0 |
| freq-rep2 | poor | 1 | `0x5e571001` | completed | 613.0 | 7458 | 730.0 | 11 | 39/234 | 146 | 589.7/19.1/4.2 | 233.5 / 309.1 | 36/21 | 1/3 | 60.0 |
| freq-rep2 | poor | 2 | `0x5e571002` | DROPPED | 359.1 | 0 | 0.0 | 0 | 23/138 | 83 | 347.8/6.8/1.5 | n/a | 22/19 | 3/6 | 60.0 |
| freq-rep2 | poor | 3 | `0x5e571003` | completed | 613.0 | 9492 | 929.1 | 14 | 39/234 | 138 | 589.7/19.1/4.2 | 233.5 / 325.8 | 31/23 | 0/0 | 60.0 |
| freq-rep2 | poor | 4 | `0x5e571004` | DROPPED | 479.0 | 678 | 84.9 | 1 | 31/186 | 105 | 468.7/6.8/1.5 | 75.6 / 75.6 | 35/21 | 4/6 | 60.0 |
| outer-rs12-6 | clean | 0 | `0x5e551000` | completed | 613.5 | 23052 | 2254.4 | 34 | 34/408 | 408 | 556.9/46.4/10.2 | 16.4 / 16.4 | 0/0 | 0/0 | 18.0 |
| outer-rs12-6 | good | 0 | `0x5e551000` | completed | 603.5 | 18984 | 1887.3 | 28 | 34/408 | 386 | 556.9/38.2/8.4 | 16.4 / 134.4 | 81/12 | 0/0 | 18.1 |
| outer-rs12-6 | good | 1 | `0x5e551001` | completed | 601.9 | 18306 | 1824.9 | 27 | 34/408 | 387 | 556.9/36.9/8.1 | 16.4 / 150.8 | 80/19 | 0/0 | 18.1 |
| outer-rs12-6 | good | 2 | `0x5e551002` | completed | 603.5 | 18984 | 1887.3 | 28 | 34/408 | 392 | 556.9/38.2/8.4 | 16.4 / 118.0 | 81/20 | 0/0 | 18.1 |
| outer-rs12-6 | good | 3 | `0x5e551003` | completed | 605.2 | 19662 | 1949.3 | 29 | 34/408 | 394 | 556.9/39.6/8.7 | 16.4 / 118.0 | 75/16 | 0/0 | 18.1 |
| outer-rs12-6 | good | 4 | `0x5e551004` | completed | 608.5 | 21018 | 2072.3 | 31 | 34/408 | 396 | 556.9/42.3/9.3 | 16.4 / 50.8 | 72/26 | 0/0 | 18.1 |
| outer-rs12-6 | moderate | 0 | `0x5e541000` | completed | 613.3 | 16272 | 1592.0 | 24 | 35/420 | 386 | 573.3/32.8/7.2 | 34.4 / 204.9 | 87/112 | 0/0 | 18.7 |
| outer-rs12-6 | moderate | 1 | `0x5e541001` | completed | 611.6 | 15594 | 1529.8 | 23 | 35/420 | 381 | 573.3/31.4/6.9 | 34.4 / 199.9 | 93/108 | 0/0 | 18.7 |
| outer-rs12-6 | moderate | 2 | `0x5e541002` | completed | 613.3 | 16272 | 1592.0 | 24 | 35/420 | 382 | 573.3/32.8/7.2 | 16.4 / 252.4 | 80/112 | 0/0 | 18.4 |
| outer-rs12-6 | moderate | 3 | `0x5e541003` | completed | 616.6 | 17628 | 1715.4 | 26 | 35/420 | 395 | 573.3/35.5/7.8 | 16.4 / 204.9 | 80/113 | 0/0 | 18.2 |
| outer-rs12-6 | moderate | 4 | `0x5e541004` | completed | 609.9 | 14916 | 1467.3 | 22 | 35/420 | 375 | 573.3/30.0/6.6 | 16.4 / 267.1 | 86/108 | 0/0 | 18.9 |
| outer-rs12-6 | poor | 0 | `0x5e571000` | completed | 601.6 | 11526 | 1149.5 | 17 | 35/420 | 360 | 573.3/23.2/5.1 | 83.6 / 268.7 | 94/128 | 0/0 | 53.2 |
| outer-rs12-6 | poor | 1 | `0x5e571001` | completed | 601.6 | 11526 | 1149.5 | 17 | 35/420 | 367 | 573.3/23.2/5.1 | 101.6 / 285.1 | 108/130 | 1/1 | 60.0 |
| outer-rs12-6 | poor | 2 | `0x5e571002` | completed | 616.3 | 10848 | 1056.1 | 16 | 36/432 | 370 | 589.7/21.8/4.8 | 139.4 / 372.0 | 100/148 | 1/1 | 60.0 |
| outer-rs12-6 | poor | 3 | `0x5e571003` | completed | 636.0 | 12204 | 1151.3 | 18 | 37/444 | 393 | 606.1/24.6/5.4 | 52.5 / 322.9 | 107/149 | 0/0 | 45.7 |
| outer-rs12-6 | poor | 4 | `0x5e571004` | completed | 632.7 | 10848 | 1028.7 | 16 | 37/444 | 380 | 606.1/21.8/4.8 | 159.1 / 388.4 | 110/145 | 0/0 | 53.2 |

## Why the outer code survives and repetition does not

Loss rates are per trial, as a share of packets decoded. They are not comparable between arms one
to one: a session that survives keeps sending through the 0 dB trough, while a session that drops
stops early.

- **The control needs all six data packets.** On Moderate it lost 41-53% of decoded packets: 13-27%
  to sync or header failure and 26-28% to payload failure. The receiver keeps good packets across
  attempts, but at that loss rate some blocks still miss a packet on six sends in a row, and four
  of five sessions dropped that way.
- **Repetition cuts payload loss but not sync or header loss.** On Moderate its payload failures
  fell to 14-18% of decoded packets, while sync or header failures were 22-30%. The preamble and
  header are not repeated, and a block still needs all six packets. Three things in this variant
  work against it, and the run does not separate them:
  - Its frames are 1.85x longer (2.52 s), so the channel estimate from the preamble ages more
    across the payload at Moderate's Doppler.
  - Its mean packet power is 1.10 dB below the control's, because the same PAPR clip and peak
    scaling act on a signal with a higher peak-to-average ratio. The noise reference is the
    control's power, so it ran at about 1.1 dB lower SNR.
  - Its replica pairs are 1100-1300 Hz apart. On Moderate's 1 ms two-tap channel the model gives a
    complex-gain correlation of 0.59 to 0.95 across that range (0.81 at 1200 Hz, about 0.65 in
    power), so a deep fade often hits both copies. This is computed from the channel model, not
    measured. On Poor, where the 2 ms delay gives 0.31 at 1200 Hz, repetition kept 3/5 up.
- **The outer code absorbs lost packets of either kind.** A packet lost to sync, header or payload
  failure is an erasure, and any six of twelve rebuild the block. On Moderate it lost 49-53% of
  decoded packets, having run through the whole trough, and still rebuilt 22-26 blocks per trial
  from 35 block transmissions, with no block pending at the cutoff. Its parity is spread over
  16.4 s, which covers many independent fades at Moderate's 0.25 Hz Doppler sigma.

## Selection rule

The rule fixed in the plan, applied in order:

1. **Moderate session survival.** outer-rs12-6 5/5, freq-rep2 2/5, matched control 1/5. The
   outer code wins.
2. **Matched Moderate net goodput.** outer-rs12-6 1579.3, matched control 709.4, freq-rep2 465.9
   bytes/min. Not needed to break a tie, and it agrees.
3. **Clean-channel airtime cost.** freq-rep2 1.70x and outer-rs12-6 1.83x the control. The outer
   code is 7.5% dearer per clean-channel byte than repetition. This is the one criterion it loses,
   and the rule ranks it last.

Poor, the stress check, agrees: 5/5 against 3/5. Good, the regression diagnostic, shows the
outer code costing 18.5% throughput against the matched control, with no survival loss (5/5). The
selected variant is `outer-rs12-6` exactly as specified in the appendix: RS(12,6) over GF(256)
with polynomial 0x11d and a systematic Cauchy generator `C[j][i] = 1 / ((6 + j) xor i)`. It runs
over six 113-byte chunks with a 4-byte packet header, at level 2, with one ARQ segment per
twelve-packet block.

## Measured limits

- **Simulation only.** There are five deterministic trials per preset per arm, ACKs are ideal, and
  the channel is a two-tap Watterson model. No RF trial was run, and five sessions give no
  statistical confidence interval worth quoting.
- **The receiver is not the production receiver.** No arm has turbo re-estimation or IR-HARQ
  combining, while the production `CoppaTransceiver::receive` has both. Adding them could change
  every arm's absolute numbers. They apply per codeword, so they would not remove sync/header
  losses.
- **The ARQ adaptation is a prototype choice.** One ARQ segment carries a whole logical block,
  the block is ACKed once and resent whole, and the initial RTO is capped at 60 s in every arm.
  A production design may choose otherwise, for example by ACKing packets inside a block.
- **The bench's bare-frame receive loses its last OFDM symbol.** Handing a frame cut at its last
  transmitted sample to the RX bandpass and `demodulate_frame` returned 1936 of 1944 level-2
  symbols. The same frame with a 1200-sample tail returned 1980 (the `CHECK bare-frame symbols`
  line). The 300-sample filter delay pushes
  the final OFDM symbol past the end of the buffer. The original `session.rs` hands
  `CoppaTransceiver::receive` exactly such bare frames, so its level-2 frames decode with 8 coded
  bits erased. This prototype adds the tail in every arm. The effect on other levels and benches
  was not measured. `docs/LIMITATIONS.md` records it.
- **The 600 s cutoff censors late failures.** A block fails on its sixth send, five RTOs after
  its first. Once the RTO has backed off to its 60 s cap, a block first sent after about 300 s
  cannot drop before 600 s, in any arm and in the original bench. The `pending` column shows how
  close each completed session was. The outer code's completed sessions ended with no block past
  its first send. The control's one completed Moderate session ended with three blocks pending,
  the worst on its third send, and repetition's two ended with two and five, also at most three.
- **Packets are received in isolation.** Each packet passes through the channel on its own buffer
  with a silent tail. The delayed tap's echo of the previous packet's last samples is dropped, and
  the tail holds noise where the next packet's preamble would really be. Packets inside a block
  are sent back to back, so this flatters sync and header detection a little, equally per packet
  in every arm.
- **Copied receive helpers.** The decode chain, the median noise fallback, the analytic signal and
  the SNR ramp are copied from `CoppaTransceiver`, `coppa_channel::watterson` and `session.rs`, not
  called. A re-run against a changed receiver has to update the copies by hand.
- **One variant per mechanism.** The frequency result is for half-band pairing on one OFDM
  symbol, with the header unprotected. The long-codeword result is for one RS(12,6) outer code
  over 1.365 s packets. Neither result covers all designs of its kind.
- **Cost on clean channels.** The outer code halves clean-channel goodput. Whether the rate loop
  should switch it on only under fading was not measured.

## Reproduce

1. Original control, from a checkout at `a6b14ef`:
   `cargo run --release -p coppa-bench --example session`. It took 4 min 31 s wall time here on a
   shared 32-core host.
2. Comparison: place the two appendix files in a directory `cop118-proto/` next to a checkout
   named `coppa` (or edit the paths in `Cargo.toml`). Copy the checkout's `Cargo.lock` into it,
   then run `cargo run --release --offline` inside `cop118-proto/`. The crate declares an empty
   `[workspace]`, so it is not a member of the coppa workspace. It runs all 48 sessions on
   parallel threads: three arms, three presets, five trials, and one clean session per arm. That
   took 5 min 6 s wall time here. It prints the `CHECK`, `PROFILE`, `ARM`, `PRESET`, `TRACE`,
   `PREF` and `POWER` lines above, then one tab-separated `TRIAL` line per session and one `SUMMARY` line per
   preset and arm. The tables in this report are those lines, unedited apart from layout.

## Appendix: prototype source

`cop118-proto/Cargo.toml`:

```toml
[package]
name = "cop118-proto"
version = "0.0.0"
edition = "2021"
publish = false

# Standalone scratch crate: not a workspace member, never shipped.
[workspace]

[dependencies]
coppa-dsp = { path = "../coppa/crates/coppa-dsp" }
coppa-codec = { path = "../coppa/crates/coppa-codec" }
coppa-protocol = { path = "../coppa/crates/coppa-protocol" }
coppa-channel = { path = "../coppa/crates/coppa-channel" }
coppa-bench = { path = "../coppa/crates/coppa-bench" }
rand = "0.10"
num-complex = "0.4"
crc = "3.2"
```

`cop118-proto/src/main.rs`:

```rust
//! COP-118 scratch prototype (throwaway measurement code, never shipped).
//!
//! Compares, on one continuous time-indexed Watterson channel per trial:
//! - `matched-control`: a logical block of six NR-coded level-2 packets,
//!   complete only when all six decode;
//! - `freq-rep2`: the same six packets, each coded BPSK symbol repeated on a
//!   second data carrier half the data band away, combined with the receiver's
//!   own per-carrier noise estimates before NR decoding;
//! - `outer-rs12-6`: a systematic RS(12,6) GF(256) erasure code across the six
//!   chunks, sent as twelve NR-coded packets, complete from any six.
//!
//! All arms drive the real `ArqTx`/`ArqRx` state machines (window 8, five
//! retransmits) through the same 20 -> 0 -> 20 dB ramp and the same per-trial
//! channel trace; each arm's physical packets use the same custom receive chain
//! (RX bandpass -> `CoppaModem::demodulate_frame` -> soft demap -> deinterleave
//! -> rate dematch -> pad pin -> NR BG2 decode -> CRC-32), with no turbo
//! re-estimation and no cross-attempt HARQ in any arm.
//!
//! Run: `cargo run --release` from this crate's directory.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use coppa_bench::scenario::{select_profile, SAMPLE_RATE};
use coppa_channel::watterson::{watterson_with_gains, WattersonPreset};
use coppa_codec::ofdm::coppa_modem::{CoppaModem, SPEED_LEVELS};
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::traits::ConstellationMapper;
use coppa_dsp::fft::FftProcessor;
use coppa_dsp::fir::{design_bandpass, Fir};
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::fec::ldpc::{pin_known_pad, rate_match, NrLdpc};
use coppa_protocol::fec::scrambler::scramble;
use coppa_protocol::modem::frame_airtime_s;
use coppa_protocol::modem::speed_levels::{
    k_used_for_level, max_payload_for_level, speed_level_components,
};
use crc::{Crc, CRC_32_ISO_HDLC};
use num_complex::Complex32;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const LEVEL: u8 = 2;
const WINDOW: u8 = 8;
const MAX_RETRANSMIT: u8 = 5;
const TURNAROUND: Duration = Duration::from_millis(150);
const SESSION_S: f64 = 600.0;
const TRIALS: usize = 5;
const CODED: usize = 1944;
const PIN_LLR: f32 = 64.0;
const LLR_CLIP: f32 = 20.0;
const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);
/// Prototype packet metadata: block id (u16 BE), packet index, packet count.
const META: usize = 4;
const K_DATA: usize = 6;
const N_OUTER: usize = 12;
/// Gain-trace rate and length (seconds) per trial.
const TRACE_FS: f64 = 100.0;
const TRACE_S: f64 = 900.0;
const RTO_CAP_S: f64 = 60.0;
/// Receiver listening margin after each frame (not charged as airtime): the
/// 601-tap RX bandpass delays the frame by 300 samples, so a buffer cut at the
/// frame's last TX sample loses the final OFDM symbol.
const RX_TAIL: usize = 1200;

fn with_tail(mut w: Vec<f32>) -> Vec<f32> {
    w.extend(std::iter::repeat_n(0.0f32, RX_TAIL));
    w
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arm {
    Control,
    Freq,
    Outer,
}

impl Arm {
    fn name(self) -> &'static str {
        match self {
            Arm::Control => "matched-control",
            Arm::Freq => "freq-rep2",
            Arm::Outer => "outer-rs12-6",
        }
    }
    fn packets(self) -> usize {
        match self {
            Arm::Outer => N_OUTER,
            _ => K_DATA,
        }
    }
}

fn preset_name(p: WattersonPreset) -> &'static str {
    match p {
        WattersonPreset::Good => "good",
        WattersonPreset::Moderate => "moderate",
        WattersonPreset::Poor => "poor",
    }
}

fn ramp_snr_db(elapsed_s: f64, total_s: f64) -> f32 {
    let half = total_s / 2.0;
    if elapsed_s <= half {
        (20.0 - 20.0 * (elapsed_s / half)) as f32
    } else {
        let t2 = (elapsed_s - half).min(half);
        (20.0 * (t2 / half)) as f32
    }
}

// ---------------------------------------------------------------- GF(256) RS

struct Gf {
    exp: [u8; 512],
    log: [u8; 256],
}

impl Gf {
    fn new() -> Self {
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];
        let mut x: u16 = 1;
        for i in 0..255 {
            exp[i] = x as u8;
            log[x as usize] = i as u8;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= 0x11d;
            }
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        Self { exp, log }
    }
    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[self.log[a as usize] as usize + self.log[b as usize] as usize]
        }
    }
    fn inv(&self, a: u8) -> u8 {
        assert!(a != 0, "GF(256) inverse of zero");
        self.exp[255 - self.log[a as usize] as usize]
    }
}

/// Systematic RS(N_OUTER, K_DATA) erasure code: generator rows 0..K are the
/// identity, rows K..N are a Cauchy matrix C[j][i] = 1 / (x_j + y_i) with
/// x_j = K + j and y_i = i (disjoint sets, so every square submatrix of
/// [I; C] used by the decoder is invertible: the code is MDS).
struct OuterCode {
    gf: Gf,
    gen: Vec<[u8; K_DATA]>,
}

impl OuterCode {
    fn new() -> Self {
        let gf = Gf::new();
        let mut gen = Vec::with_capacity(N_OUTER);
        for r in 0..N_OUTER {
            let mut row = [0u8; K_DATA];
            if r < K_DATA {
                row[r] = 1;
            } else {
                let xj = r as u8;
                for (i, c) in row.iter_mut().enumerate() {
                    *c = gf.inv(xj ^ i as u8);
                }
            }
            gen.push(row);
        }
        Self { gf, gen }
    }

    fn encode(&self, data: &[Vec<u8>]) -> Vec<Vec<u8>> {
        assert_eq!(data.len(), K_DATA);
        let len = data[0].len();
        (0..N_OUTER)
            .map(|r| {
                (0..len)
                    .map(|b| {
                        (0..K_DATA)
                            .fold(0u8, |acc, i| acc ^ self.gf.mul(self.gen[r][i], data[i][b]))
                    })
                    .collect()
            })
            .collect()
    }

    /// Recover the K data chunks from any K distinct packets; `None` with fewer.
    fn decode(&self, avail: &[(usize, &[u8])]) -> Option<Vec<Vec<u8>>> {
        if avail.len() < K_DATA {
            return None;
        }
        let use_ = &avail[..K_DATA];
        let len = use_[0].1.len();
        // Gauss-Jordan inverse of the K x K submatrix of generator rows.
        let mut m: Vec<[u8; K_DATA]> = use_.iter().map(|(r, _)| self.gen[*r]).collect();
        let mut inv: Vec<[u8; K_DATA]> = (0..K_DATA)
            .map(|i| {
                let mut e = [0u8; K_DATA];
                e[i] = 1;
                e
            })
            .collect();
        for col in 0..K_DATA {
            let piv = (col..K_DATA).find(|&r| m[r][col] != 0)?;
            m.swap(col, piv);
            inv.swap(col, piv);
            let s = self.gf.inv(m[col][col]);
            for c in 0..K_DATA {
                m[col][c] = self.gf.mul(m[col][c], s);
                inv[col][c] = self.gf.mul(inv[col][c], s);
            }
            for r in 0..K_DATA {
                if r != col && m[r][col] != 0 {
                    let f = m[r][col];
                    for c in 0..K_DATA {
                        m[r][c] ^= self.gf.mul(f, m[col][c]);
                        inv[r][c] ^= self.gf.mul(f, inv[col][c]);
                    }
                }
            }
        }
        Some(
            (0..K_DATA)
                .map(|i| {
                    (0..len)
                        .map(|b| {
                            (0..K_DATA)
                                .fold(0u8, |acc, k| acc ^ self.gf.mul(inv[i][k], use_[k].1[b]))
                        })
                        .collect()
                })
                .collect(),
        )
    }
}

// ---------------------------------------------------------------- PHY

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fail {
    /// `demodulate_frame` returned nothing: preamble sync or header FEC failed.
    SyncOrHeader,
    /// A header decoded but disagrees with what this arm sends.
    Header,
    /// NR decoder did not converge, or the CRC-32 failed.
    Payload,
}

struct Phy {
    modem: CoppaModem,
    rx_bpf: Fir,
    ldpc: NrLdpc,
    il: BlockInterleaver,
    mapper: Box<dyn ConstellationMapper>,
    k_used: usize,
    papr_db: f32,
    dps: usize,
    max_payload: usize,
}

fn header(len: usize, seq: u8, codewords: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: LEVEL,
        seq_num: seq,
        payload_len: len as u16,
        codewords,
    }
}

fn median(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 1.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = s.len() / 2;
    if s.len() % 2 == 0 {
        (s[mid - 1] + s[mid]) / 2.0
    } else {
        s[mid]
    }
}

impl Phy {
    fn new() -> Self {
        let profile = select_profile(LEVEL);
        assert_eq!(profile.phy_mode, 0, "level 2 is an HF profile");
        let rx_bpf = Fir::new(design_bandpass(
            601,
            profile.sample_rate as f32,
            250.0,
            2850.0,
        ));
        let (mapper, _) = speed_level_components(LEVEL).expect("level 2");
        let k_used = k_used_for_level(LEVEL).expect("level 2");
        let il = BlockInterleaver::new(CODED, profile.data_carriers);
        let papr_db = SPEED_LEVELS
            .iter()
            .find(|s| s.level == LEVEL)
            .expect("level 2")
            .papr_target_db;
        let modem = CoppaModem::new(profile, 1);
        let dps = modem.data_carriers_per_symbol();
        assert!(dps % 2 == 0, "pairing needs an even data-carrier count");
        Self {
            modem,
            rx_bpf,
            ldpc: NrLdpc::new(),
            il,
            mapper,
            k_used,
            papr_db,
            dps,
            max_payload: max_payload_for_level(LEVEL).expect("level 2"),
        }
    }

    /// Same chain as `CoppaTransceiver::transmit` for one RV0 codeword.
    fn encode_symbols(&self, payload: &[u8]) -> Vec<Complex32> {
        assert!(payload.len() <= self.max_payload);
        let mut pc = payload.to_vec();
        pc.extend_from_slice(&CRC32.checksum(payload).to_be_bytes());
        let mut bits = Vec::with_capacity(NrLdpc::INFO_LEN);
        for &b in &pc {
            for s in (0..8).rev() {
                bits.push((b >> s) & 1);
            }
        }
        bits.resize(self.k_used, 0);
        bits.resize(NrLdpc::INFO_LEN, 0);
        scramble(&mut bits);
        let mother = self.ldpc.encode(&bits);
        let coded = rate_match::rate_match(&mother, self.k_used, CODED, 0);
        self.mapper.map_bits(&self.il.interleave(&coded))
    }

    /// Stream position of coded symbol `k` and of its frequency replica:
    /// OFDM symbol `k / h`, data slot `k % h` in the lower half of the data
    /// slots and the same slot `h` positions higher.
    fn rep_positions(&self, k: usize) -> (usize, usize) {
        let h = self.dps / 2;
        let p = (k / h) * self.dps + k % h;
        (p, p + h)
    }

    fn tx(&self, payload: &[u8], seq: u8, arm: Arm) -> Vec<f32> {
        let c = self.encode_symbols(payload);
        match arm {
            Arm::Freq => {
                let nsym = CODED.div_ceil(self.dps / 2);
                let mut s = vec![Complex32::new(1.0, 0.0); nsym * self.dps];
                for (k, &sym) in c.iter().enumerate() {
                    let (p, r) = self.rep_positions(k);
                    s[p] = sym;
                    s[r] = sym;
                }
                // `codewords: 2` makes the receiver demodulate 2 x 1944 symbols.
                self.modem
                    .modulate_mapped(&header(payload.len(), seq, 2), &s, self.papr_db)
            }
            _ => self
                .modem
                .modulate_mapped(&header(payload.len(), seq, 1), &c, self.papr_db),
        }
    }

    fn demod(
        &self,
        samples: &[f32],
        arm: Arm,
    ) -> Result<(CoppaHeader, Vec<Complex32>, Vec<f32>), Fail> {
        let filtered = self.rx_bpf.filter_block(samples);
        let (hdr, syms, nvs, _) = self
            .modem
            .demodulate_frame(&filtered)
            .ok_or(Fail::SyncOrHeader)?;
        let want_cw = if arm == Arm::Freq { 2 } else { 1 };
        if hdr.speed_level != LEVEL
            || hdr.codewords != want_cw
            || hdr.payload_len as usize > self.max_payload
        {
            return Err(Fail::Header);
        }
        Ok((hdr, syms, nvs))
    }

    /// Maximum-ratio combine each coded symbol with its replica. For BPSK the
    /// combined symbol's LLR equals the sum of the two per-carrier LLRs.
    fn combine(&self, syms: &[Complex32], nvs: &[f32]) -> (Vec<Complex32>, Vec<f32>) {
        let fb = median(nvs);
        let nv_at = |i: usize| match nvs.get(i) {
            Some(&v) if v > 1e-6 => v,
            _ => fb,
        };
        let mut out_s = Vec::with_capacity(CODED);
        let mut out_v = Vec::with_capacity(CODED);
        for k in 0..CODED {
            let (p, r) = self.rep_positions(k);
            match (syms.get(p), syms.get(r)) {
                (Some(&y1), Some(&y2)) => {
                    let (w1, w2) = (1.0 / nv_at(p), 1.0 / nv_at(r));
                    out_s.push((y1 * w1 + y2 * w2) / (w1 + w2));
                    out_v.push(1.0 / (w1 + w2));
                }
                (Some(&y1), None) => {
                    out_s.push(y1);
                    out_v.push(nv_at(p));
                }
                _ => {
                    out_s.push(Complex32::new(0.0, 0.0));
                    out_v.push(fb);
                }
            }
        }
        (out_s, out_v)
    }

    /// Same chain as `CoppaTransceiver::receive_single_codeword` minus turbo
    /// re-estimation and the IR-HARQ accumulator.
    fn decode(&self, syms: &[Complex32], nvs: &[f32], payload_len: usize) -> Result<Vec<u8>, Fail> {
        let fb = median(nvs);
        let mut llrs = Vec::with_capacity(CODED);
        for (i, &s) in syms.iter().take(CODED).enumerate() {
            let nv = match nvs.get(i) {
                Some(&v) if v > 1e-6 => v,
                _ => fb,
            };
            llrs.extend(self.mapper.demap_soft(s, nv));
        }
        llrs.truncate(CODED);
        llrs.resize(CODED, 0.0);
        for l in &mut llrs {
            *l = l.clamp(-LLR_CLIP, LLR_CLIP);
        }
        let de = self.il.deinterleave(&llrs);
        let payload_bits = (payload_len + 4) * 8;
        if payload_bits > self.k_used {
            return Err(Fail::Header);
        }
        let mut dm = rate_match::rate_dematch(&de, self.k_used, CODED, 0, NrLdpc::MOTHER_LEN);
        pin_known_pad(&mut dm, payload_bits, self.k_used, PIN_LLR);
        let (_, mut bits, converged, _) = self.ldpc.decode_soft_stats(&dm);
        scramble(&mut bits);
        if !converged {
            return Err(Fail::Payload);
        }
        let total = payload_len + 4;
        let mut bytes = Vec::with_capacity(total);
        for ch in bits.chunks(8) {
            if ch.len() == 8 && bytes.len() < total {
                bytes.push(
                    ch.iter()
                        .enumerate()
                        .fold(0u8, |a, (i, &b)| a | ((b & 1) << (7 - i))),
                );
            }
        }
        let (p, c) = bytes.split_at(payload_len);
        let rx_crc = u32::from_be_bytes(c.try_into().map_err(|_| Fail::Payload)?);
        if CRC32.checksum(p) != rx_crc {
            return Err(Fail::Payload);
        }
        Ok(p.to_vec())
    }

    fn rx(&self, samples: &[f32], arm: Arm) -> Result<Vec<u8>, Fail> {
        let (hdr, syms, nvs) = self.demod(samples, arm)?;
        let (syms, nvs) = if arm == Arm::Freq {
            self.combine(&syms, &nvs)
        } else {
            (syms, nvs)
        };
        self.decode(&syms, &nvs, hdr.payload_len as usize)
    }
}

// ---------------------------------------------------------------- channel

fn analytic(x: &[f32]) -> Vec<Complex32> {
    let n = x.len();
    let fft = FftProcessor::new(n);
    let input: Vec<Complex32> = x.iter().map(|&v| Complex32::new(v, 0.0)).collect();
    let mut s = fft.forward(&input);
    for (k, v) in s.iter_mut().enumerate().skip(1) {
        if k < n.div_ceil(2) {
            *v *= 2.0;
        } else if !(n % 2 == 0 && k == n / 2) {
            *v = Complex32::new(0.0, 0.0);
        }
    }
    fft.inverse(&s)
}

/// One continuous Watterson realization per trial: each tap's complex gain is
/// generated once at TRACE_FS by the crate's own generator (`watterson_with_gains`
/// over a silent TRACE_S-long signal), then linearly interpolated onto the
/// absolute 48 kHz time of every transmitted sample. Never renormalized.
struct Trace {
    taps: Vec<(f32, usize, Vec<Complex32>)>,
}

impl Trace {
    fn new(preset: WattersonPreset, seed: u64) -> Self {
        let cfg = preset.config();
        let n = (TRACE_S * TRACE_FS) as usize;
        let (_, gains) = watterson_with_gains(&vec![0.0f32; n], TRACE_FS as f32, &cfg, seed);
        Self {
            taps: gains
                .into_iter()
                .map(|(tap, g)| {
                    (
                        tap.power.max(0.0).sqrt(),
                        (tap.delay_s * SAMPLE_RATE as f32).round() as usize,
                        g,
                    )
                })
                .collect(),
        }
    }

    fn gain(&self, tap: usize, t: f64) -> Complex32 {
        let x = t * TRACE_FS;
        let i = x.floor() as usize;
        let f = (x - i as f64) as f32;
        let g = &self.taps[tap].2;
        assert!(i + 1 < g.len(), "gain trace too short for t = {t}");
        g[i] * (1.0 - f) + g[i + 1] * f
    }

    fn apply(&self, x: &[f32], t0: f64) -> Vec<f32> {
        let a = analytic(x);
        let n = x.len();
        let mut out = vec![0.0f32; n];
        for (ti, (amp, d, _)) in self.taps.iter().enumerate() {
            for i in *d..n {
                let t = t0 + i as f64 / SAMPLE_RATE as f64;
                out[i] += (a[i - d] * self.gain(ti, t) * *amp).re;
            }
        }
        out
    }
}

// ---------------------------------------------------------------- session

#[derive(Default, Clone, Debug)]
struct Rec {
    dropped: bool,
    elapsed_s: f64,
    bytes: usize,
    blocks_delivered: usize,
    block_tx: usize,
    packets_tx: usize,
    data_air_s: f64,
    ack_air_s: f64,
    turn_s: f64,
    latencies: Vec<f64>,
    fail_sync: usize,
    fail_header: usize,
    fail_payload: usize,
    verify_fail: usize,
    /// Packets actually handed to the receiver (held packets are skipped).
    decodes: usize,
    /// Blocks still unacknowledged when the session ended, and the highest
    /// transmit count among them (5 retransmits = 6 sends is the budget).
    pending_end: usize,
    pending_max_tx: u8,
    rto_end_s: f64,
}

fn block_data(seed: u64, id: u16, chunk: usize) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(seed ^ (id as u64) ^ 0xACE_0000);
    (0..K_DATA * chunk).map(|_| rng.random::<u8>()).collect()
}

fn make_packets(arm: Arm, code: &OuterCode, id: u16, data: &[u8], chunk: usize) -> Vec<Vec<u8>> {
    let chunks: Vec<Vec<u8>> = data.chunks(chunk).map(|c| c.to_vec()).collect();
    let bodies = if arm == Arm::Outer {
        code.encode(&chunks)
    } else {
        chunks
    };
    let n = bodies.len() as u8;
    bodies
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let mut p = vec![(id >> 8) as u8, id as u8, i as u8, n];
            p.extend_from_slice(&b);
            p
        })
        .collect()
}

struct Ctx<'a> {
    arm: Arm,
    phy: &'a Phy,
    code: &'a OuterCode,
    trace: Option<&'a Trace>,
    seed: u64,
    p_ref: f32,
    chunk: usize,
}

/// Transmit every packet of one logical block back to back at `*now`, then
/// report whether the receiver can reconstruct the block (using packets it
/// kept from earlier attempts of the same block id).
fn send_block(
    cx: &Ctx,
    id: u16,
    data: &[u8],
    store: &mut HashMap<u16, Vec<Option<Vec<u8>>>>,
    now: &mut Instant,
    start: Instant,
    rec: &mut Rec,
) -> bool {
    let sr = SAMPLE_RATE as f64;
    let pk = make_packets(cx.arm, cx.code, id, data, cx.chunk);
    let n = pk.len();
    for (i, p) in pk.iter().enumerate() {
        let wave = cx.phy.tx(p, id as u8, cx.arm);
        let t0 = now.duration_since(start).as_secs_f64();
        let dur = wave.len() as f64 / sr;
        *now += Duration::from_secs_f64(dur);
        rec.packets_tx += 1;
        rec.data_air_s += dur;
        let slot = store.entry(id).or_insert_with(|| vec![None; n]);
        if slot[i].is_some() {
            continue; // already held; airtime still charged
        }
        rec.decodes += 1;
        let wave = with_tail(wave);
        let rxw = match cx.trace {
            Some(tr) => {
                let faded = tr.apply(&wave, t0);
                let snr = ramp_snr_db(t0 + dur / 2.0, SESSION_S);
                let start_sample = (t0 * sr).round() as u64;
                coppa_channel::awgn_ref_seeded(
                    &faded,
                    snr,
                    cx.p_ref,
                    SAMPLE_RATE as f32,
                    cx.seed ^ 0x5555_5555_5555_5555 ^ start_sample,
                )
            }
            None => wave,
        };
        match cx.phy.rx(&rxw, cx.arm) {
            Ok(bytes) => {
                let ok_meta = bytes.len() == p.len()
                    && bytes[0] == (id >> 8) as u8
                    && bytes[1] == id as u8
                    && bytes[2] as usize == i
                    && bytes[3] as usize == n;
                if ok_meta {
                    slot[i] = Some(bytes[META..].to_vec());
                } else {
                    rec.fail_header += 1;
                }
            }
            Err(Fail::SyncOrHeader) => rec.fail_sync += 1,
            Err(Fail::Header) => rec.fail_header += 1,
            Err(Fail::Payload) => rec.fail_payload += 1,
        }
    }
    let slot = store.get(&id).expect("inserted above");
    let recovered: Option<Vec<u8>> = if cx.arm == Arm::Outer {
        let avail: Vec<(usize, &[u8])> = slot
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_deref().map(|b| (i, b)))
            .collect();
        cx.code.decode(&avail).map(|c| c.concat())
    } else if slot.iter().all(|s| s.is_some()) {
        Some(slot.iter().flat_map(|s| s.clone().unwrap()).collect())
    } else {
        None
    };
    match recovered {
        Some(r) if r == data => {
            store.remove(&id);
            true
        }
        Some(_) => {
            rec.verify_fail += 1;
            false
        }
        None => false,
    }
}

fn block_airtime_s(phy: &Phy, arm: Arm, chunk: usize) -> f64 {
    let wave = phy.tx(&vec![0u8; META + chunk], 0, arm);
    arm.packets() as f64 * wave.len() as f64 / SAMPLE_RATE as f64
}

fn run_session(arm: Arm, preset: Option<WattersonPreset>, seed: u64, p_ref: f32) -> Rec {
    let phy = Phy::new();
    let code = OuterCode::new();
    let profile = select_profile(LEVEL);
    let frame_air = frame_airtime_s(LEVEL, &profile).expect("level 2");
    let trace = preset.map(|p| Trace::new(p, seed ^ 0x3333_3333_3333_3333));
    let chunk = phy.max_payload - META;
    let block_air = block_airtime_s(&phy, arm, chunk);
    // Same formula as `arq::rto_floor`, with the arm's block airtime in place
    // of one frame's, capped at the ARQ's 60 s backoff ceiling.
    let initial_rto =
        (WINDOW as f64 * block_air + 2.0 * TURNAROUND.as_secs_f64() + frame_air).min(RTO_CAP_S);
    let config = ArqConfig::new(WINDOW, MAX_RETRANSMIT, Duration::from_secs_f64(initial_rto))
        .expect("valid ARQ config")
        .with_airtime_params(LEVEL, TURNAROUND, profile);
    let mut tx = ArqTx::new(config);
    let mut rxq = ArqRx::new(WINDOW);
    let cx = Ctx {
        arm,
        phy: &phy,
        code: &code,
        trace: trace.as_ref(),
        seed,
        p_ref,
        chunk,
    };

    let start = Instant::now();
    let mut now = start;
    let mut rec = Rec::default();
    let mut next_id: u16 = 0;
    let mut seq_id: HashMap<u8, u16> = HashMap::new();
    let mut submitted: HashMap<u16, Instant> = HashMap::new();
    let mut store: HashMap<u16, Vec<Option<Vec<u8>>>> = HashMap::new();
    let mut pending: std::collections::HashSet<u8> = std::collections::HashSet::new();
    let frame_air_d = Duration::from_secs_f64(frame_air);

    let ack = |seq: u8,
               data: Vec<u8>,
               now: &mut Instant,
               rec: &mut Rec,
               tx: &mut ArqTx,
               rxq: &mut ArqRx,
               seq_id: &HashMap<u8, u16>,
               submitted: &mut HashMap<u16, Instant>| {
        let delivered = rxq.receive(seq, data);
        for (dseq, d) in delivered {
            rec.bytes += d.len();
            rec.blocks_delivered += 1;
            if let Some(t) = seq_id.get(&dseq).and_then(|id| submitted.remove(id)) {
                rec.latencies.push(now.duration_since(t).as_secs_f64());
            }
        }
        *now += TURNAROUND;
        let (an, bm) = rxq.ack_info();
        *now += frame_air_d;
        *now += TURNAROUND;
        rec.ack_air_s += frame_air;
        rec.turn_s += 2.0 * TURNAROUND.as_secs_f64();
        tx.process_ack(an, bm, *now);
    };

    while now.duration_since(start).as_secs_f64() < SESSION_S && !rec.dropped {
        if tx.can_send() {
            let id = next_id;
            next_id = next_id.wrapping_add(1);
            let data = block_data(seed, id, chunk);
            let seq = tx.send(data.clone(), now).expect("window has room");
            seq_id.insert(seq, id);
            submitted.insert(id, now);
            pending.insert(seq);
            rec.block_tx += 1;
            if send_block(&cx, id, &data, &mut store, &mut now, start, &mut rec) {
                pending.remove(&seq);
                ack(
                    seq,
                    data,
                    &mut now,
                    &mut rec,
                    &mut tx,
                    &mut rxq,
                    &seq_id,
                    &mut submitted,
                );
            }
        } else {
            now += Duration::from_millis(500);
        }
        for seq in tx.get_retransmits(now) {
            tx.mark_retransmitted(seq, now)
                .expect("seq came from get_retransmits");
            if tx.is_failed(seq) {
                rec.dropped = true;
                continue;
            }
            let data = tx.get_segment_data(seq).expect("in flight").to_vec();
            let id = seq_id[&seq];
            rec.block_tx += 1;
            if send_block(&cx, id, &data, &mut store, &mut now, start, &mut rec) {
                pending.remove(&seq);
                ack(
                    seq,
                    data,
                    &mut now,
                    &mut rec,
                    &mut tx,
                    &mut rxq,
                    &seq_id,
                    &mut submitted,
                );
            }
        }
    }
    rec.elapsed_s = now.duration_since(start).as_secs_f64();
    rec.pending_end = pending.len();
    rec.pending_max_tx = pending
        .iter()
        .filter_map(|&q| tx.transmit_count(q))
        .max()
        .unwrap_or(0);
    rec.rto_end_s = tx.rto().as_secs_f64();
    rec
}

// ---------------------------------------------------------------- checks

fn self_checks(phy: &Phy, code: &OuterCode) {
    let chunk = phy.max_payload - META;
    let mut rng = StdRng::seed_from_u64(118);
    // 1. Clean exact-byte recovery, every arm.
    for arm in [Arm::Control, Arm::Freq, Arm::Outer] {
        for _ in 0..3 {
            let p: Vec<u8> = (0..META + chunk).map(|_| rng.random::<u8>()).collect();
            let w = with_tail(phy.tx(&p, 7, arm));
            assert_eq!(phy.rx(&w, arm), Ok(p), "{} clean loopback", arm.name());
        }
    }
    println!("CHECK clean exact-byte recovery: PASS (3 packets x 3 arms)");
    // 2. Replica alignment: on a clean frame both copies of every coded symbol
    //    carry the transmitted BPSK sign.
    let p: Vec<u8> = (0..META + chunk).map(|_| rng.random::<u8>()).collect();
    let c = phy.encode_symbols(&p);
    let (_, syms, _) = phy
        .demod(&with_tail(phy.tx(&p, 1, Arm::Freq)), Arm::Freq)
        .expect("clean demod");
    let mut mismatch = 0;
    for (k, s) in c.iter().enumerate() {
        let (a, b) = phy.rep_positions(k);
        if syms[a].re.signum() != s.re.signum() || syms[b].re.signum() != s.re.signum() {
            mismatch += 1;
        }
    }
    assert_eq!(mismatch, 0, "replica alignment");
    println!(
        "CHECK replica alignment: PASS ({} coded symbols, {} demodulated symbols)",
        c.len(),
        syms.len()
    );
    // 3. Replica rescue: wipe one copy of every symbol (huge noise variance on
    //    the lower half), decode must still succeed from the other half.
    let (_, syms, mut nvs) = phy
        .demod(&with_tail(phy.tx(&p, 1, Arm::Freq)), Arm::Freq)
        .expect("demod");
    let mut syms = syms;
    for k in 0..CODED {
        let (a, _) = phy.rep_positions(k);
        syms[a] = Complex32::new(0.0, 0.0);
        nvs[a] = 1e6;
    }
    let (cs, cv) = phy.combine(&syms, &nvs);
    assert_eq!(
        phy.decode(&cs, &cv, p.len()),
        Ok(p.clone()),
        "one-half erasure"
    );
    println!("CHECK combining with one replica erased: PASS");
    // 4. Outer code: every 6-of-12 subset decodes exactly; 5 packets fail.
    let data: Vec<Vec<u8>> = (0..K_DATA)
        .map(|_| (0..chunk).map(|_| rng.random::<u8>()).collect())
        .collect();
    let pk = code.encode(&data);
    let mut subsets = 0;
    for mask in 0u32..(1 << N_OUTER) {
        if mask.count_ones() as usize != K_DATA {
            continue;
        }
        let avail: Vec<(usize, &[u8])> = (0..N_OUTER)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| (i, pk[i].as_slice()))
            .collect();
        assert_eq!(
            code.decode(&avail).as_ref(),
            Some(&data),
            "subset {mask:#x}"
        );
        assert!(code.decode(&avail[..K_DATA - 1]).is_none());
        subsets += 1;
    }
    println!("CHECK RS(12,6): PASS ({subsets} six-packet subsets exact, five-packet sets refused)");
    // 5. Channel: a unit single-tap trace is the identity (analytic signal ok).
    let x: Vec<f32> = (0..4801).map(|i| (i as f32 * 0.37).sin() * 0.5).collect();
    let unit = Trace {
        taps: vec![(1.0, 0, vec![Complex32::new(1.0, 0.0); 100])],
    };
    let y = unit.apply(&x, 0.0);
    let err = x
        .iter()
        .zip(&y)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(err < 1e-3, "identity channel max error {err}");
    println!("CHECK identity channel: PASS (max abs error {err:.2e})");
    // 6. Bare-frame truncation: the RX bandpass delay drops the last OFDM symbol
    //    of a buffer cut at the frame's last TX sample.
    let p: Vec<u8> = (0..META + chunk).map(|_| rng.random::<u8>()).collect();
    let w = phy.tx(&p, 1, Arm::Control);
    let bare = phy.demod(&w, Arm::Control).expect("bare demod").1.len();
    let tail = phy
        .demod(&with_tail(w), Arm::Control)
        .expect("tail demod")
        .1
        .len();
    println!("CHECK bare-frame symbols: {bare} bare vs {tail} with a {RX_TAIL}-sample tail (1944 needed)");
}

fn trace_stats(preset: WattersonPreset) {
    let sigma = preset.config().doppler_sigma_hz() as f64;
    let lags = [0.5f64, 1.0, 2.0, 5.0];
    let n = (600.0 * TRACE_FS) as usize;
    let seeds = 20u64;
    let mut pw = 0.0f64;
    let mut ac = vec![0.0f64; lags.len()];
    for s in 0..seeds {
        let tr = Trace::new(preset, 0xC0118 + s);
        for (_, _, g) in &tr.taps {
            let p: f64 = g[..n].iter().map(|c| c.norm_sqr() as f64).sum::<f64>() / n as f64;
            pw += p;
            for (li, &lag) in lags.iter().enumerate() {
                let l = (lag * TRACE_FS) as usize;
                let c: f64 = (0..n - l)
                    .map(|i| (g[i] * g[i + l].conj()).re as f64)
                    .sum::<f64>()
                    / (n - l) as f64;
                ac[li] += c / p;
            }
        }
    }
    let m = (seeds * 2) as f64;
    print!(
        "TRACE {}: mean tap power {:.3} (expect 1.000); autocorrelation",
        preset_name(preset),
        pw / m
    );
    for (li, &lag) in lags.iter().enumerate() {
        let th = (-2.0 * std::f64::consts::PI.powi(2) * sigma * sigma * lag * lag).exp();
        print!(" | {lag}s {:.3} vs {:.3}", ac[li] / m, th);
    }
    println!();
}

fn main() {
    let phy = Phy::new();
    let code = OuterCode::new();
    self_checks(&phy, &code);

    let profile = select_profile(LEVEL);
    let chunk = phy.max_payload - META;
    let spacing = profile.sample_rate as f64 / profile.fft_size as f64;
    let active = profile.total_active_carriers();
    let sep_hz = active as f64 / 2.0 * spacing;
    println!(
        "PROFILE fft {} cp {} spacing {:.3} Hz, active carriers {}, data/OFDM symbol {}, replica separation ~{:.0} Hz (half the active band)",
        profile.fft_size, profile.cp_samples, spacing, active, phy.dps, sep_hz
    );
    let frame_air = frame_airtime_s(LEVEL, &profile).expect("level 2");
    for arm in [Arm::Control, Arm::Freq, Arm::Outer] {
        let b = block_airtime_s(&phy, arm, chunk);
        println!(
            "ARM {}: {} packets/block, packet {:.3} s, block {:.3} s, block bytes {}, initial RTO {:.1} s",
            arm.name(),
            arm.packets(),
            b / arm.packets() as f64,
            b,
            K_DATA * chunk,
            (WINDOW as f64 * b + 0.3 + frame_air).min(RTO_CAP_S)
        );
    }
    for preset in [
        WattersonPreset::Good,
        WattersonPreset::Moderate,
        WattersonPreset::Poor,
    ] {
        let cfg = preset.config();
        let tau = cfg.taps[1].delay_s as f64;
        let sigma = cfg.doppler_sigma_hz() as f64;
        let rho = |t: f64| (-2.0 * std::f64::consts::PI.powi(2) * sigma * sigma * t * t).exp();
        println!(
            "PRESET {}: delay {:.1} ms, sigma {:.2} Hz, |rho_f(sep)| = {:.2}, rho_t(1 s) {:.3}, rho_t(5 s) {:.3}",
            preset_name(preset),
            tau * 1e3,
            sigma,
            (std::f64::consts::PI * sep_hz * tau).cos().abs(),
            rho(1.0),
            rho(5.0)
        );
        trace_stats(preset);
    }

    // Fixed noise reference: mean power of one clean control packet.
    let p_ref = coppa_channel::mean_power(&phy.tx(&vec![0x5Au8; META + chunk], 0, Arm::Control));
    println!("PREF {p_ref:.6e}");
    for arm in [Arm::Freq, Arm::Outer] {
        let p = coppa_channel::mean_power(&phy.tx(&vec![0x5Au8; META + chunk], 0, arm));
        println!(
            "POWER {}: mean packet power {:.6e} ({:+.2} dB vs control)",
            arm.name(),
            p,
            10.0 * (p / p_ref).log10()
        );
    }

    let arms = [Arm::Control, Arm::Freq, Arm::Outer];
    let presets = [
        WattersonPreset::Good,
        WattersonPreset::Moderate,
        WattersonPreset::Poor,
    ];
    let mut jobs: Vec<(Arm, Option<WattersonPreset>, usize, u64)> = Vec::new();
    for &arm in &arms {
        jobs.push((arm, None, 0, 0x5E55_1000));
        for &p in &presets {
            for t in 0..TRIALS {
                jobs.push((
                    arm,
                    Some(p),
                    t,
                    0x5E55_1000_u64 ^ ((p as u64) << 16) ^ t as u64,
                ));
            }
        }
    }
    let results: Vec<((Arm, Option<WattersonPreset>, usize, u64), Rec)> = std::thread::scope(|s| {
        let hs: Vec<_> = jobs
            .iter()
            .map(|&j| s.spawn(move || (j, run_session(j.0, j.1, j.3, p_ref))))
            .collect();
        hs.into_iter()
            .map(|h| h.join().expect("session thread"))
            .collect()
    });

    println!("\nTRIAL\tarm\tpreset\ttrial\tseed\tstatus\telapsed_s\tbytes\tbytes_per_min\tblocks\tblock_tx\tpackets\tdata_air_s\tack_air_s\tturn_s\tlat_med_s\tlat_max_s\tfail_sync_hdr\tfail_header\tfail_payload\tverify_fail\tdecodes\tpending_end\tpending_max_tx\trto_end_s");
    for ((arm, preset, t, seed), r) in &results {
        let mut l = r.latencies.clone();
        l.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = if l.is_empty() {
            f64::NAN
        } else {
            l[l.len() / 2]
        };
        let max = l.last().copied().unwrap_or(f64::NAN);
        println!(
            "TRIAL\t{}\t{}\t{}\t{:#x}\t{}\t{:.1}\t{}\t{:.1}\t{}\t{}\t{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}",
            arm.name(),
            preset.map(preset_name).unwrap_or("clean"),
            t,
            seed,
            if r.dropped { "DROPPED" } else { "completed" },
            r.elapsed_s,
            r.bytes,
            r.bytes as f64 / (r.elapsed_s / 60.0),
            r.blocks_delivered,
            r.block_tx,
            r.packets_tx,
            r.data_air_s,
            r.ack_air_s,
            r.turn_s,
            med,
            max,
            r.fail_sync,
            r.fail_header,
            r.fail_payload,
            r.verify_fail,
            r.decodes,
            r.pending_end,
            r.pending_max_tx,
            r.rto_end_s
        );
    }

    println!("\nSUMMARY\tpreset\tarm\tsurvival\tmean_bytes_per_min\tthroughput_cost_pct\tlat_med_s\tlat_max_s");
    for p in [
        None,
        Some(WattersonPreset::Good),
        Some(WattersonPreset::Moderate),
        Some(WattersonPreset::Poor),
    ] {
        let mean_of = |arm: Arm| {
            let rs: Vec<&Rec> = results
                .iter()
                .filter(|((a, pp, _, _), _)| *a == arm && pp.map(preset_name) == p.map(preset_name))
                .map(|(_, r)| r)
                .collect();
            let gp: f64 = rs
                .iter()
                .map(|r| r.bytes as f64 / (r.elapsed_s / 60.0))
                .sum::<f64>()
                / rs.len() as f64;
            let surv = rs.iter().filter(|r| !r.dropped).count();
            let mut l: Vec<f64> = rs
                .iter()
                .flat_map(|r| r.latencies.iter().copied())
                .collect();
            l.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (gp, surv, rs.len(), l)
        };
        let (ctrl, _, _, _) = mean_of(Arm::Control);
        for arm in arms {
            let (gp, surv, n, l) = mean_of(arm);
            let cost = if ctrl > 0.0 {
                format!("{:.1}", 100.0 * (1.0 - gp / ctrl))
            } else {
                "undefined".to_string()
            };
            let med = if l.is_empty() {
                f64::NAN
            } else {
                l[l.len() / 2]
            };
            println!(
                "SUMMARY\t{}\t{}\t{}/{}\t{:.1}\t{}\t{:.1}\t{:.1}",
                p.map(preset_name).unwrap_or("clean"),
                arm.name(),
                surv,
                n,
                gp,
                cost,
                med,
                l.last().copied().unwrap_or(f64::NAN)
            );
        }
    }
}
```
