# COP-236 black-box transfer record

This page records the evidence for COP-236. Two release `coppad`
processes on the COP-214 black-box cable sometimes stopped a 2048-byte
transfer after the first 512-byte segment and ran to the 600 s cap. It
gives the cause, the timer change, the release matrix the change was
accepted on, and why a stalled run logs `TX audio is clipping`.

## Cause

coppad runs a connected session as stop-and-wait ARQ. After it keys a
multi-codeword OFDM data frame, `EventLoop::set_session_segment_floor`
(`crates/coppa-daemon/src/event_loop.rs`) sets that frame's retransmit
timeout from the audio it keyed:

`data + 2 x turnaround + ACK = 9.476 + 2 x 0.750 + 1.365 = 12.341 s`

for a level-2 eight-codeword frame with coppad's default radio timing
(`turnaround_ms = 500`, `ptt_pre_delay_ms = 50`, `ptt_tail_delay_ms = 200`).
That budget has no term for the peer's decode, its RX-to-TX hold, the
audio path in each direction, or the sender's own decode of the ACK. On
the black-box cable the sender finished decoding the ACK 12.60 to 12.69 s
after the send, over 15 frames the research measured in ARDOP and
VARA-style runs. The timeout is checked by a 500 ms poll. When a poll
tick lands in the 0.26 to 0.35 s gap, coppad resends the 9.5 s frame just
before it processes an ACK it already has.

The duplicate breaks the half-duplex exchange. The peer ACKs the
duplicate while the sender is already keying the next segment, so the
cable mutes the peer for part of that segment and it is lost. Its timer
has doubled, both stations send KEEPALIVE together, and after two
unanswered windows the session falls back to the 16-FSK floor (COP-211).
A 512-byte segment then has to go as 18-byte floor fragments (COP-222),
about 18 s per exchange, which cannot finish inside the 600 s cap.

## Baseline

The base is `e25f643abf70f6bfafef24d995cc01b0dadeb480`, the `origin/main`
this ticket started from. The plan reused the COP-236 research baseline
at that same commit rather than rerun it (Phase 1, step 1). That research
ran in a 2-CPU cgroup (`cpu.max = 200000 100000`):

| Profile | Runs | Result |
|---|---|---|
| ARDOP | 16 | 11 `complete`, 5 stalled (3 of 11 with default logging, 2 of 5 with debug logging) |
| VARA-style | 7 | 7 `complete` |
| KISS | 6 | 6 `datagram` |

The research stopped each stalled run at 300 s, so those runs have no
`cap` row. The COP-236 report recorded natural `cap` rows with 512 and
517 bytes delivered. The planning run at this commit completed 3
of 3 ARDOP runs.

Excerpt from the research's stalled debug-logged ARDOP runs
`ardopdbg-r02` and `ardopdbg-r03`, which share one timeline (station A
unless marked B, absolute times):

```text
03:54:47.993 DEBUG Session data frame transmitted seq=0 frame=1 max=4
03:54:58.147 B     Holding TX for RX->TX turnaround wait_ms=470
03:54:58.622 B     TX audio level duration_ms=1365                 (ACK seq 0)
03:55:00.547 DEBUG Retransmitting session frame seq=0 attempt=2 mode=Ofdm { level: 2 }
03:55:10.508 DEBUG Session data frame transmitted seq=1 frame=2
03:55:11.198 B     TX audio level duration_ms=1365                 (ACK of the duplicate, over seq 1)
03:55:35.529 DEBUG Retransmitting session frame seq=1 attempt=2
03:55:43.366 A     Sending session KEEPALIVE
03:55:43.470 B     Sending session KEEPALIVE
03:56:25.350 A     TX audio level peak_dbfs=0.0 rms_dbfs=-3.0 duration_ms=8000   (first floor frame)
```

The retransmission of seq 0 is 12.554 s after its send. The ACK of seq 0
was decoded at about 12.6 s, so A sends seq 1 as soon as the duplicate
ends.

## Change

`SESSION_ACK_PROCESSING_ALLOWANCE` adds one second to the OFDM segment
floor of a multi-codeword or ID-carrying frame. The level-2 example
moves from 12.341 s to 13.341 s, at least 0.65 s past the latest measured
ACK decode. The single-codeword reset, the floor branch, the refused
key-up path, the shared RTT estimator, the 60 s cap and the floor
fragment deadlines are unchanged. A frame whose ACK is lost is resent
later than before. Each expiry doubles the whole timeout, the allowance
included, so the level-2 example's successive timeouts are 13.3, 26.7
and 53.4 s and then the 60 s cap, against 12.3, 24.7 and 49.4 s before.
The allowance is a fixed second. It does not scale with level, codeword
count or host load.

Two unit tests in `event_loop.rs` model the stalled schedule on paused
Tokio time with a level-2 eight-codeword frame:

- `cop236_multi_codeword_ack_arrives_before_retry` polls at 12.554 s and
  12.688 s, requires no due segment and no backoff, then applies the ACK
  at 12.689 s and requires the segment retired.
- `cop236_multi_codeword_missing_ack_still_retries` withholds the ACK and
  requires the segment not due 1 ms before its deadline, due at it, one
  backoff, and a timeout between 13.339 s and 13.35 s.

Before the change, `cargo test -p coppa-daemon --lib cop236_` failed both:

```text
test event_loop::tests::telemetry::cop236_multi_codeword_missing_ack_still_retries ... FAILED
test event_loop::tests::telemetry::cop236_multi_codeword_ack_arrives_before_retry ... FAILED
RTO 12.34125s leaves under 0.65 s past the latest measured ACK decode
the frame was due 12.554s after the send, before its ACK (RTO 12.34125s)
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 625 filtered out
```

After the change both pass, as do the 37 existing timer controls
(`cargo test -p coppa-daemon --lib -- rto multi_codeword cop111 station_id`)
and the 15 floor recovery tests
(`cargo test --profile ci -p coppa-daemon --lib cop222_`).

## Release matrix

The matrix ran on the patched tree in a Linux container with a 6-CPU
cgroup quota (`cpu.max = 600000 100000`, `nproc` 32, shared host load
average 6 to 10). A 2-CPU quota could not be set here, so every
qualifying run was pinned to two dedicated cores with `taskset -c`.

```sh
cargo build --release -p coppa-bench -p coppa-daemon \
  --features coppa-bench/cpal-backend,coppa-daemon/cpal-backend,coppa-daemon/ardop,coppa-daemon/kiss-tnc
```

`target/release/coppad --version` printed `coppad 0.18.0`. Its SHA-256,
`5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594`, is the
`binary_sha256` of every row below. Each run had fresh directories and
default logging:

```sh
env -u RUST_LOG TMPDIR="$run_dir/tmp" timeout --kill-after=10s 720s taskset -c <cores> \
  target/release/coppa-bench compare blackbox \
  --modem "tools/compare/$profile.toml" --channel wgn --snr 30 \
  --size 2048 --trials 1 --jobs 1 --seed-base 0x17000000 \
  --out-dir "$run_dir/results"
```

A row passes only with outcome `complete` (ARDOP, VARA-style) or
`datagram` (KISS), 2048 bytes delivered, `exact = true`, channel `wgn`,
SNR 30, seed `0x17000000`, cap 600 s and `payload_crc32 = 0x6887c6a5`.
ARDOP runs 1 to 3 ran one after another on cores 2,3. After that, two
streams ran at once on separate core pairs: cores 2,3 took ARDOP 4 to 10
and VARA-style 1 to 5, and cores 4,5 took KISS 1 to 10 and VARA-style 6 to
10. No build or test job overlapped a qualifying run.

Result: ARDOP 10 of 10 `complete`, VARA-style 10 of 10 `complete` and KISS 10 of 10 `datagram`, each with all 2048 bytes exact. No row ended `cap`, and every row counted 0 starved and 0 overflow ticks. One ARDOP run index needed a second attempt after a probe failure, listed below.

| Profile | Run | Cores | Started (UTC) | Outcome | Delivered | Exact | Trial wall s | Starved | Overflow |
|---|---|---|---|---|---|---|---|---|---|
| ARDOP | 1 | 2,3 | 08:36:29 | `complete` | 2048 | true | 58.694 | 0 | 0 |
| ARDOP | 2 | 2,3 | 08:38:09 | `complete` | 2048 | true | 58.701 | 0 | 0 |
| ARDOP | 3 | 2,3 | 08:39:37 | `complete` | 2048 | true | 58.685 | 0 | 0 |
| ARDOP | 4 | 2,3 | 08:41:05 | `complete` | 2048 | true | 58.660 | 0 | 0 |
| ARDOP | 5 | 2,3 | 08:42:32 | `complete` | 2048 | true | 58.704 | 0 | 0 |
| ARDOP | 6 | 2,3 | 08:44:00 | `complete` | 2048 | true | 58.563 | 0 | 0 |
| ARDOP | 7 | 2,3 | 08:45:29 | `complete` | 2048 | true | 58.615 | 0 | 0 |
| ARDOP | 8 | 2,3 | 08:46:58 | `complete` | 2048 | true | 58.582 | 0 | 0 |
| ARDOP | 9 | 2,3 | 08:48:25 | `complete` | 2048 | true | 58.670 | 0 | 0 |
| ARDOP | 10 | 2,3 | 08:49:54 | `complete` | 2048 | true | 58.684 | 0 | 0 |
| VARA-style | 1 | 2,3 | 08:51:22 | `complete` | 2048 | true | 58.565 | 0 | 0 |
| VARA-style | 2 | 2,3 | 08:52:49 | `complete` | 2048 | true | 58.616 | 0 | 0 |
| VARA-style | 3 | 2,3 | 08:54:16 | `complete` | 2048 | true | 58.597 | 0 | 0 |
| VARA-style | 4 | 2,3 | 08:55:44 | `complete` | 2048 | true | 58.641 | 0 | 0 |
| VARA-style | 5 | 2,3 | 08:57:12 | `complete` | 2048 | true | 58.554 | 0 | 0 |
| VARA-style | 6 | 4,5 | 08:49:26 | `complete` | 2048 | true | 58.564 | 0 | 0 |
| VARA-style | 7 | 4,5 | 08:50:53 | `complete` | 2048 | true | 58.562 | 0 | 0 |
| VARA-style | 8 | 4,5 | 08:52:20 | `complete` | 2048 | true | 58.581 | 0 | 0 |
| VARA-style | 9 | 4,5 | 08:53:48 | `complete` | 2048 | true | 58.602 | 0 | 0 |
| VARA-style | 10 | 4,5 | 08:55:15 | `complete` | 2048 | true | 58.607 | 0 | 0 |
| KISS | 1 | 4,5 | 08:41:33 | `datagram` | 2048 | true | 25.542 | 0 | 0 |
| KISS | 2 | 4,5 | 08:42:21 | `datagram` | 2048 | true | 25.547 | 0 | 0 |
| KISS | 3 | 4,5 | 08:43:08 | `datagram` | 2048 | true | 25.534 | 0 | 0 |
| KISS | 4 | 4,5 | 08:43:55 | `datagram` | 2048 | true | 25.537 | 0 | 0 |
| KISS | 5 | 4,5 | 08:44:42 | `datagram` | 2048 | true | 25.550 | 0 | 0 |
| KISS | 6 | 4,5 | 08:45:29 | `datagram` | 2048 | true | 25.537 | 0 | 0 |
| KISS | 7 | 4,5 | 08:46:17 | `datagram` | 2048 | true | 25.514 | 0 | 0 |
| KISS | 8 | 4,5 | 08:47:04 | `datagram` | 2048 | true | 25.570 | 0 | 0 |
| KISS | 9 | 4,5 | 08:47:51 | `datagram` | 2048 | true | 25.547 | 0 | 0 |
| KISS | 10 | 4,5 | 08:48:39 | `datagram` | 2048 | true | 25.535 | 0 | 0 |

Full CSV rows, with the profile and the fresh-run index first:

```csv
profile,run,modem,adapter,version_reply,binary_sha256,mode,channel,snr_db,size_bytes,trial,seed,ref_power_a,ref_power_b,cap_s,outcome,connect_s,delivered_bytes,exact,transfer_s,net_bytes_per_min,drops,wall_s,cable_latency_ms,starved_ticks,overflow_ticks,payload_crc32
coppad-ardop,1,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.135,2048,true,51.559,2383.296,0,58.694,355,0,0,0x6887c6a5
coppad-ardop,2,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.156,2048,true,51.545,2383.936,0,58.701,355,0,0,0x6887c6a5
coppad-ardop,3,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.160,2048,true,51.525,2384.872,0,58.685,355,0,0,0x6887c6a5
coppad-ardop,4,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.145,2048,true,51.515,2385.343,0,58.660,375,0,0,0x6887c6a5
coppad-ardop,5,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.184,2048,true,51.520,2385.086,0,58.704,355,0,0,0x6887c6a5
coppad-ardop,6,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.172,2048,true,51.392,2391.056,0,58.563,355,0,0,0x6887c6a5
coppad-ardop,7,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.158,2048,true,51.457,2387.998,0,58.615,355,0,0,0x6887c6a5
coppad-ardop,8,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.133,2048,true,51.449,2388.374,0,58.582,355,0,0,0x6887c6a5
coppad-ardop,9,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.200,2048,true,51.471,2387.383,0,58.670,355,0,0,0x6887c6a5
coppad-ardop,10,coppad-ardop,ardop,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.143,2048,true,51.542,2384.096,0,58.684,355,0,0,0x6887c6a5
coppad,1,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.077,2048,true,51.488,2386.571,0,58.565,355,0,0,0x6887c6a5
coppad,2,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.054,2048,true,51.562,2383.173,0,58.616,355,0,0,0x6887c6a5
coppad,3,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.103,2048,true,51.494,2386.292,0,58.597,355,0,0,0x6887c6a5
coppad,4,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.037,2048,true,51.604,2381.204,0,58.641,355,0,0,0x6887c6a5
coppad,5,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.090,2048,true,51.463,2387.720,0,58.554,355,0,0,0x6887c6a5
coppad,6,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.065,2048,true,51.499,2386.073,0,58.564,355,0,0,0x6887c6a5
coppad,7,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.057,2048,true,51.505,2385.802,0,58.562,355,0,0,0x6887c6a5
coppad,8,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.082,2048,true,51.499,2386.065,0,58.581,355,0,0,0x6887c6a5
coppad,9,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.070,2048,true,51.532,2384.547,0,58.602,355,0,0,0x6887c6a5
coppad,10,coppad,vara,VERSION Coppa 0.18.0,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.092,2048,true,51.515,2385.316,0,58.607,355,0,0,0x6887c6a5
coppad-kiss,1,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019260,600,datagram,,2048,true,25.542,4810.880,0,25.542,355,0,0,0x6887c6a5
coppad-kiss,2,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019260,600,datagram,,2048,true,25.547,4809.944,0,25.547,355,0,0,0x6887c6a5
coppad-kiss,3,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019260,600,datagram,,2048,true,25.534,4812.479,0,25.534,355,0,0,0x6887c6a5
coppad-kiss,4,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.537,4811.799,0,25.537,355,0,0,0x6887c6a5
coppad-kiss,5,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.550,4809.475,0,25.550,355,0,0,0x6887c6a5
coppad-kiss,6,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.537,4811.916,0,25.537,355,0,0,0x6887c6a5
coppad-kiss,7,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.514,4816.176,0,25.514,355,0,0,0x6887c6a5
coppad-kiss,8,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.570,4805.645,0,25.570,355,0,0,0x6887c6a5
coppad-kiss,9,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019260,600,datagram,,2048,true,25.547,4810.000,0,25.547,355,0,0,0x6887c6a5
coppad-kiss,10,coppad-kiss,kiss,,5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.535,4812.147,0,25.535,355,0,0,0x6887c6a5
```

A run whose integrity probe failed before any coppad started records no
trial. Each such run index was retried once:

- ARDOP run 2 at 08:37:56 on cores 2,3

Batches that do not count:

- 08:31:39 UTC, unpinned on 6 CPUs: ARDOP run 1 was `complete` (2048
  bytes, exact, trial wall 58.599 s). The batch was stopped during run 2
  to switch to two-core pinning.
- 08:34:04 UTC, pinned to cores 0,1: run 1's integrity probe failed
  (correlation 0.998959, level -0.01 dB, no trial recorded).
- The serial stream on cores 2,3 was stopped after ARDOP run 3 so the
  remaining runs could use two core pairs. ARDOP run 4 had already started
  and finished normally, so it counts.

### Patched trace

A fresh release build in the resumed container produced `coppad 0.18.0`
with SHA-256 `35da66656c770bedfbff98c520f60106545748abc51b4f2b7b93ecb85a3f458f`.
One serial smoke run per profile used the release command above, the same
seed, WGN 30 dB and 2048-byte payload, with fresh output and scratch
directories. ARDOP returned `complete`, 2048 exact, in 58.662 s;
VARA-style returned `complete`, 2048 exact, in 58.576 s; and KISS
returned `datagram`, 2048 exact, in 25.519 s. The fresh binary hash is
different from the earlier matrix hash, so these smoke rows are separate
evidence and do not replace any of the 30 matrix rows.

One debug-logged ARDOP run on the patched binary
(`RUST_LOG=info,coppad::event_loop=debug,coppa_protocol=debug`, cores 2,3,
08:34:24 UTC, before the qualifying matrix) was `complete` with 2048
bytes, exact, trial wall 58.643 s. Station A's log shows the four
segments with no retransmission, no 8000 ms floor frame and no clipping
warning:

```text
08:35:03.159584 DEBUG Session data frame transmitted seq=0 frame=1 max=4
08:35:15.828959 DEBUG Holding TX for RX->TX turnaround wait_ms=286   (ACK seq 0 decoded, 12.669 s)
08:35:16.168911 DEBUG Session data frame transmitted seq=1 frame=2 max=4
08:35:28.804616 DEBUG Holding TX for RX->TX turnaround wait_ms=310   (12.636 s)
08:35:29.168698 DEBUG Session data frame transmitted seq=2 frame=3 max=4
08:35:41.788684 DEBUG Holding TX for RX->TX turnaround wait_ms=306   (12.620 s)
08:35:42.148560 DEBUG Session data frame transmitted seq=3 frame=4 max=4
```

The ACKs were decoded at 12.62 to 12.67 s, inside the new 13.341 s
timeout. In the stalled baseline the poll resent seq 0 at 12.554 s.

## Floor burst and the TX clipping warning

A stalled run logs `TX audio is clipping before it leaves Coppa: this is a
Coppa bug` at each station from its first floor frame on. The warning
marks the floor fallback. It does not cause the stall, and no sample
exceeds full scale.

In the research's stalled runs, the 8000 ms TX level lines read
`peak_dbfs=0.0 rms_dbfs=-3.0 duration_ms=8000`. Station A's first one is
at 03:56:25.350 in the excerpt above. The TX clipping warning logged with
them at the same station read `peak_dbfs=0.0` and `clipped_percent` 8.7
to 9.1. The 8000 ms burst is a 16-FSK floor frame: `FRAME_SAMPLES` is
exactly 8 s (`crates/coppa-protocol/src/modem/floor.rs:45`).

- `FloorFrame::encode` builds the frame by concatenating
  `FloorFsk::tone_waveform` renders (`crates/coppa-protocol/src/modem/floor.rs:127`).
  Each render is the real part of a unit-amplitude tone
  (`crates/coppa-codec/src/floor.rs:101`). Nothing normalizes the result.
- Every OFDM frame is peak-normalized to `TX_PEAK = 0.5`, -6.02 dBFS
  (`crates/coppa-codec/src/ofdm/coppa_modem.rs:251` and `:776`).
- The daemon meters every keyed waveform before it plays it
  (`report_tx_level`). `CLIP_THRESHOLD` is 0.99
  (`crates/coppa-daemon/src/level_meter.rs:39`), and `tx_state` reports
  `Clipping` when any sample reaches it (`level_meter.rs:303`).
- A unit sine peaks at 0.0 dBFS, has an RMS of -3.01 dBFS, and spends
  `1 - (2/pi) asin(0.99)` = 9.01 % of its samples at or above 0.99. That
  matches the logged values.

So every floor frame trips the TX clipping warning, in a stalled run or a
healthy floor session. A completed OFDM run never keys a floor frame and
logs no such warning. The floor frame also leaves about 6 dB above the
OFDM peak and 13 dB above the OFDM RMS, which contradicts the -6.02 dBFS
level the meter's message and `docs/OPERATING.md` describe. COP-236 takes
the explanation route. Changing the floor waveform's level or the meter
is separate work, tracked by the follow-up filed with COP-236 (Linear
issue ID `56e0272a-76d7-4fa4-8436-42af6e828684`).

## Not shown

- Which commit made the race visible. COP-211's floor fallback and
  COP-222's 18-byte fragments turn a duplicate frame into a stall that
  outlasts the cap. The timer budget predates both.
- Rates under heavier load, such as `--jobs 3` or busy CPU loops.
  Decode time grows with load and the allowance covers 0.65 s of it past
  the latest measured decode.
- A natural `cap` row from a stalled run at this base.
