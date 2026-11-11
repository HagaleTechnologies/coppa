> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## Engine processing and ideal frame goodput (COP-131)

Run `cargo bench --bench throughput` for `engine_encode/1` through
`engine_encode/9` and `engine_decode/1` through `engine_decode/9`, followed by
the existing DSP component cases. These IDs start new Criterion histories.
Each engine case uses the same uncompressed 32-byte ASCII payload,
`0123456789ABCDEF0123456789ABCDEF`. Default level-derived routing selects
`hf_standard` at levels 1–4 and `vhf_wide` at levels 5–9, at 48000 Hz with
long CP. This workload does not force every level onto HF.

Criterion's `time` is local encode or decode processing latency, including
normal result allocation and disposal. Its `thrpt` is 32 application bytes
divided by that processing time, displayed in binary byte units. Core setup,
loopback verification and the frame report are outside the timed loops.
Decode uses pre-encoded clean audio. CPU results depend on the host and build;
there is no minimum throughput threshold.

Before timing, the command verifies exact payload recovery at every level,
then prints nine rows under `Engine frame metrics: ideal clean loopback`.
Frame airtime is actual encoded sample count divided by configured sample
rate. Ideal frame goodput is recovered application bytes times eight divided
by that airtime. CPU time is not part of this denominator. For example, an
implementation run on 2026-11-04 printed this row (seconds and bits/second):

```text
level,profile,payload_bytes,frame_samples,frame_airtime_s,ideal_frame_goodput_bps
1,hf_standard,32,65520,1.365000,187.55
```

These fixed short frames are not maximum-capacity measurements. Framing and
padding can give adjacent levels equal goodput. The report excludes channel
losses, ARQ, radio turnaround and session overhead; it does not measure
connected-session latency or real-radio goodput. Use the `coppa-bench` command
above for loss-weighted channel measurements, and the connected-session
measurements later in this document for session behavior.

For a quick all-level smoke run rather than a stable CPU baseline:

```sh
cargo bench --bench throughput -- engine --warm-up-time 0.1 --measurement-time 0.2 --sample-size 10 --noplot
```

`cargo bench --bench throughput -- --list` lists all eighteen engine cases
and eight component cases. `-- --test` exercises them without collecting
statistics. An engine filter still prints the complete verified frame table.
Any fixture encode/decode error, empty frame or payload mismatch aborts setup
with the failing level, before any successful table is printed.

