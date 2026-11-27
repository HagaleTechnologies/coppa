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

The qualifying matrix is the second attempt of the remediation round, on
one release build of the patched tree (`7d07cc3`). The first matrix and
the round's first attempt each had an integrity probe fail, which AC1
counts as a failed matrix. Both are listed under "Attempts that do not
count" below. The build and every run used a Linux container with a
6-CPU cgroup quota (`cpu.max = 600000 100000`, `nproc` 32) on a shared
host: load average 6 to 11, and every core 20 to 57% busy with other work
just before this attempt. A 2-CPU quota could not be set here, because
`/sys/fs/cgroup` is read-only.

```sh
cargo build --release -p coppa-bench -p coppa-daemon \
  --features coppa-bench/cpal-backend,coppa-daemon/cpal-backend,coppa-daemon/ardop,coppa-daemon/kiss-tnc
```

`target/release/coppad --version` printed `coppad 0.18.0`. Its SHA-256,
`3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a`, is the
`binary_sha256` of every row below. Each run had fresh directories and
default logging:

```sh
env -u RUST_LOG TMPDIR="$run_dir/tmp" timeout --kill-after=10s 720s \
  target/release/coppa-bench compare blackbox \
  --modem "tools/compare/$profile.toml" --channel wgn --snr 30 \
  --size 2048 --trials 1 --jobs 1 --seed-base 0x17000000 \
  --out-dir "$run_dir/results"
```

A row passes only with exit status 0, outcome `complete` (ARDOP,
VARA-style) or `datagram` (KISS), 2048 bytes delivered, `exact = true`,
channel `wgn`, SNR 30, seed `0x17000000`, cap 600 s,
`payload_crc32 = 0x6887c6a5`, and 0 starved and 0 overflow ticks. The
script never retried a run index, and after any failed check, a failed
probe included, neither stream started another run; a run already in
flight would have finished. Two streams ran at
once from 12:52:16 UTC: stream A took ARDOP 1 to 10 and then KISS 1 to 5,
and stream B took VARA-style 1 to 10 and then KISS 6 to 10. The runs were
not pinned to cores. All three probe failures seen on this host were in
runs pinned to two cores, so this attempt dropped the pinning; that link
is suggestive, not shown. The two streams shared the 6-CPU quota, so
each pair could use more CPU than the 2-CPU cgroup where the baseline
stalled (see "Not shown"). No build or test job overlapped a qualifying
run.

Result: ARDOP 10 of 10 `complete`, VARA-style 10 of 10 `complete` and
KISS 10 of 10 `datagram`, each with exit status 0 and all 2048 bytes
exact. Every run's harness log reported its integrity probe passed with
correlation 1.000000, level +0.00 dB and a cable lead of 355 ms or 375 ms
(the CSV's `cable_latency_ms`). No row ended `cap`, and
every row counted 0 starved and 0 overflow ticks.

| Profile | Run | Stream | Started (UTC) | Exit | Outcome | Delivered | Exact | Trial wall s | Starved | Overflow |
|---|---|---|---|---|---|---|---|---|---|---|
| ARDOP | 1 | A | 12:52:16 | 0 | `complete` | 2048 | true | 58.686 | 0 | 0 |
| ARDOP | 2 | A | 12:53:44 | 0 | `complete` | 2048 | true | 58.570 | 0 | 0 |
| ARDOP | 3 | A | 12:55:10 | 0 | `complete` | 2048 | true | 58.654 | 0 | 0 |
| ARDOP | 4 | A | 12:56:38 | 0 | `complete` | 2048 | true | 58.621 | 0 | 0 |
| ARDOP | 5 | A | 12:58:05 | 0 | `complete` | 2048 | true | 58.665 | 0 | 0 |
| ARDOP | 6 | A | 12:59:32 | 0 | `complete` | 2048 | true | 58.688 | 0 | 0 |
| ARDOP | 7 | A | 13:00:59 | 0 | `complete` | 2048 | true | 58.609 | 0 | 0 |
| ARDOP | 8 | A | 13:02:26 | 0 | `complete` | 2048 | true | 58.631 | 0 | 0 |
| ARDOP | 9 | A | 13:03:53 | 0 | `complete` | 2048 | true | 58.658 | 0 | 0 |
| ARDOP | 10 | A | 13:05:20 | 0 | `complete` | 2048 | true | 59.021 | 0 | 0 |
| VARA-style | 1 | B | 12:52:16 | 0 | `complete` | 2048 | true | 58.601 | 0 | 0 |
| VARA-style | 2 | B | 12:53:43 | 0 | `complete` | 2048 | true | 58.579 | 0 | 0 |
| VARA-style | 3 | B | 12:55:10 | 0 | `complete` | 2048 | true | 58.572 | 0 | 0 |
| VARA-style | 4 | B | 12:56:37 | 0 | `complete` | 2048 | true | 58.593 | 0 | 0 |
| VARA-style | 5 | B | 12:58:04 | 0 | `complete` | 2048 | true | 58.568 | 0 | 0 |
| VARA-style | 6 | B | 12:59:31 | 0 | `complete` | 2048 | true | 58.620 | 0 | 0 |
| VARA-style | 7 | B | 13:00:58 | 0 | `complete` | 2048 | true | 58.649 | 0 | 0 |
| VARA-style | 8 | B | 13:02:25 | 0 | `complete` | 2048 | true | 58.611 | 0 | 0 |
| VARA-style | 9 | B | 13:03:52 | 0 | `complete` | 2048 | true | 58.555 | 0 | 0 |
| VARA-style | 10 | B | 13:05:19 | 0 | `complete` | 2048 | true | 59.018 | 0 | 0 |
| KISS | 1 | A | 13:06:48 | 0 | `datagram` | 2048 | true | 25.700 | 0 | 0 |
| KISS | 2 | A | 13:07:35 | 0 | `datagram` | 2048 | true | 25.562 | 0 | 0 |
| KISS | 3 | A | 13:08:22 | 0 | `datagram` | 2048 | true | 28.026 | 0 | 0 |
| KISS | 4 | A | 13:09:12 | 0 | `datagram` | 2048 | true | 25.624 | 0 | 0 |
| KISS | 5 | A | 13:09:58 | 0 | `datagram` | 2048 | true | 25.620 | 0 | 0 |
| KISS | 6 | B | 13:06:47 | 0 | `datagram` | 2048 | true | 25.726 | 0 | 0 |
| KISS | 7 | B | 13:07:34 | 0 | `datagram` | 2048 | true | 25.561 | 0 | 0 |
| KISS | 8 | B | 13:08:21 | 0 | `datagram` | 2048 | true | 27.801 | 0 | 0 |
| KISS | 9 | B | 13:09:11 | 0 | `datagram` | 2048 | true | 25.631 | 0 | 0 |
| KISS | 10 | B | 13:09:57 | 0 | `datagram` | 2048 | true | 25.599 | 0 | 0 |

Full CSV rows, with the profile and the fresh-run index first:

```csv
profile,run,modem,adapter,version_reply,binary_sha256,mode,channel,snr_db,size_bytes,trial,seed,ref_power_a,ref_power_b,cap_s,outcome,connect_s,delivered_bytes,exact,transfer_s,net_bytes_per_min,drops,wall_s,cable_latency_ms,starved_ticks,overflow_ticks,payload_crc32
coppad-ardop,1,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.156,2048,true,51.530,2384.613,0,58.686,355,0,0,0x6887c6a5
coppad-ardop,2,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.124,2048,true,51.446,2388.543,0,58.570,355,0,0,0x6887c6a5
coppad-ardop,3,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.151,2048,true,51.503,2385.879,0,58.654,355,0,0,0x6887c6a5
coppad-ardop,4,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.116,2048,true,51.505,2385.789,0,58.621,355,0,0,0x6887c6a5
coppad-ardop,5,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.136,2048,true,51.529,2384.677,0,58.665,355,0,0,0x6887c6a5
coppad-ardop,6,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.172,2048,true,51.516,2385.280,0,58.688,355,0,0,0x6887c6a5
coppad-ardop,7,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.163,2048,true,51.447,2388.495,0,58.609,355,0,0,0x6887c6a5
coppad-ardop,8,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.134,2048,true,51.498,2386.130,0,58.631,355,0,0,0x6887c6a5
coppad-ardop,9,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.176,2048,true,51.482,2386.865,0,58.658,355,0,0,0x6887c6a5
coppad-ardop,10,coppad-ardop,ardop,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.198,2048,true,51.824,2371.108,0,59.021,355,0,0,0x6887c6a5
coppad,1,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.066,2048,true,51.535,2384.398,0,58.601,355,0,0,0x6887c6a5
coppad,2,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.061,2048,true,51.518,2385.188,0,58.579,355,0,0,0x6887c6a5
coppad,3,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.072,2048,true,51.500,2386.040,0,58.572,355,0,0,0x6887c6a5
coppad,4,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.042,2048,true,51.550,2383.686,0,58.593,355,0,0,0x6887c6a5
coppad,5,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.079,2048,true,51.488,2386.555,0,58.568,355,0,0,0x6887c6a5
coppad,6,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.073,2048,true,51.547,2383.854,0,58.620,355,0,0,0x6887c6a5
coppad,7,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.167,2048,true,51.483,2386.817,0,58.649,355,0,0,0x6887c6a5
coppad,8,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.064,2048,true,51.547,2383.845,0,58.611,355,0,0,0x6887c6a5
coppad,9,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.075,2048,true,51.480,2386.955,0,58.555,355,0,0,0x6887c6a5
coppad,10,coppad,vara,VERSION Coppa 0.18.0,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.023498,0.020689,600,complete,7.152,2048,true,51.867,2369.153,0,59.018,355,0,0,0x6887c6a5
coppad-kiss,1,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019260,600,datagram,,2048,true,25.700,4781.380,0,25.700,355,0,0,0x6887c6a5
coppad-kiss,2,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.562,4807.059,0,25.562,375,0,0,0x6887c6a5
coppad-kiss,3,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,28.026,4384.423,0,28.026,355,0,0,0x6887c6a5
coppad-kiss,4,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.624,4795.507,0,25.624,355,0,0,0x6887c6a5
coppad-kiss,5,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019243,600,datagram,,2048,true,25.620,4796.166,0,25.620,355,0,0,0x6887c6a5
coppad-kiss,6,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019260,600,datagram,,2048,true,25.726,4776.491,0,25.726,355,0,0,0x6887c6a5
coppad-kiss,7,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.561,4807.259,0,25.561,355,0,0,0x6887c6a5
coppad-kiss,8,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,27.801,4419.943,0,27.801,355,0,0,0x6887c6a5
coppad-kiss,9,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019264,600,datagram,,2048,true,25.631,4794.164,0,25.631,355,0,0,0x6887c6a5
coppad-kiss,10,coppad-kiss,kiss,,3a91780c82cecd2475d492b12a1b236a7823ebfb62a99d216b3393e6e61ff43a,HF_STANDARD,wgn,30,2048,0,0x17000000,0.019621,0.019243,600,datagram,,2048,true,25.599,4800.130,0,25.599,355,0,0,0x6887c6a5
```

Attempts that do not count:

- The round's first attempt, 12:43:53 UTC, on the same binary, with the
  same two streams pinned to cores 2,3 and 4,5. ARDOP 1 to 4 and
  VARA-style 1 to 3 passed. VARA-style run 4, started at 12:48:15, failed
  its integrity probe (correlation 0.998959, level -0.01 dB, 0 starved, 0
  overflow), so neither stream started another run. ARDOP run 4, started
  at the same moment, finished `complete` with 2048 bytes exact. In the
  kept `probe.raw`, the
  probe's first 960-sample block (20 ms) arrived at capture offset 960,
  about 335 ms ahead of the rest. The other 479,040 samples arrived
  bit-exact at the usual 355 ms lead. The analysis aligns on the bulk and
  scores the first block as silence. Every sample arrived bit-exact, but
  the first block arrived early. The suspected cause is the probe child's
  sink letting its first write out before the output stream had built its
  latency. COP-253 tracks this probe race and is to confirm the cause.
- The first matrix, from 08:36:29 UTC, on binary
  `5d6bdf085634d75e21006f3441f75840e34635ddbeabcf0f59a69967ac0ae594`.
  Its 30 kept rows, listed in this page as of commit `7d07cc3`, were all
  exact: ARDOP 10 `complete`, VARA-style 10 `complete` and KISS 10
  `datagram`. ARDOP ran on cores 2,3, first serially and then beside a
  second stream on cores 4,5. ARDOP run 2, started at 08:37:56, failed its
  probe with the same correlation, 0.998959, and that run index was
  retried at 08:38:09. AC1 counts a failed probe as a failed matrix, so validation
  rejected this matrix.
- 08:31:39 UTC, unpinned on 6 CPUs: ARDOP run 1 was `complete` (2048
  bytes, exact, trial wall 58.599 s). The batch was stopped during run 2
  to switch to two-core pinning.
- 08:34:04 UTC, pinned to cores 0,1: run 1's integrity probe failed
  (correlation 0.998959, level -0.01 dB, no trial recorded).

### Patched trace

A fresh release build in the resumed container produced `coppad 0.18.0`
with SHA-256 `35da66656c770bedfbff98c520f60106545748abc51b4f2b7b93ecb85a3f458f`.
One serial smoke run per profile used the release command above, the same
seed, WGN 30 dB and 2048-byte payload, with fresh output and scratch
directories. ARDOP returned `complete`, 2048 exact, in 58.662 s;
VARA-style returned `complete`, 2048 exact, in 58.576 s; and KISS
returned `datagram`, 2048 exact, in 25.519 s. The fresh binary hash is
different from both matrix hashes, so these smoke rows are separate
evidence and do not replace any of the 30 matrix rows.

One debug-logged ARDOP run on the patched binary
(`RUST_LOG=info,coppad::event_loop=debug,coppa_protocol=debug`, cores 2,3,
08:34:24 UTC, before the first matrix) was `complete` with 2048
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
- The patched binary under the baseline's 2-CPU cgroup. The qualifying
  matrix ran unpinned, two streams sharing a 6-CPU quota on a busy shared
  host, so each pair could use more than 2 CPUs. Decode time grows as CPU
  shrinks, so these 30 rows are weaker evidence for a 2-CPU host than a
  2-CPU run would be. Every transfer in the earlier pinned runs (two
  cores each) delivered 2048 bytes exactly, but those runs do not count,
  as listed above.
