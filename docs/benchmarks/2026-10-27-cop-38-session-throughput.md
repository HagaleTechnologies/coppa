> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-38: connected-session stop-and-wait throughput (simulated)

COP-38 (0.3.0, ADR-010) puts connected-session data through `ArqTx`/`ArqRx`
with one frame in flight: every data frame carries a 7-byte `Reliable`
transport header and is answered by a MAC `Ack` frame before the next frame
keys. This section records what that costs on a clean link.

**Method.** `coppa-daemon`'s `cop60_turnaround::cable_loop_session_throughput_report`
(`#[ignore]`; run with `cargo test -p coppa-daemon --lib --features
websocket,cpal-backend -- --ignored cable_loop_session_throughput_report
--nocapture`). Two real `run()` loops (`VK3ABC` calls `W1AW`) over the
half-duplex cable simulator: real encode and streaming decode, tokio virtual
time, `DaemonConfig::default()` (`ptt_pre_delay_ms = 50`, `ptt_tail_delay_ms
= 200`, `turnaround_ms = 500`, `arq_enabled = false`), level 2
`HF_STANDARD`, no loss. One 2000-byte write; the time runs from the write to
the last byte reaching the peer's host. The `main` baseline is the COP-38
plan's run of the same harness on unmodified `65d1a3b`.

| Payload (2000 B) | `main` (0.2.0) frames | `main` rate | COP-38 data frames (+ ACKs) | COP-38 time | COP-38 rate | ratio |
|---|---|---|---|---|---|---|
| random bytes | 67 | 16.6 B/s | 47 (+47) | 179.92 s | 11.1 B/s | 0.67× |
| B2F-like text | 39 | 28.3 B/s | 29 (+29) | 110.08 s | 18.2 B/s | 0.64× |

The COP-38 figures come from the final code (this run); the sender keyed
exactly once per data frame and the receiver once per ACK, so no frame was
retransmitted. Fewer frames than `main` despite the extra header is the
`chunk_to_fit` bisection (COP-38 Phase 1): a rejected optimistic chunk is now
refined to the largest prefix that fits instead of being halved.

**Caveats.** The analytic model gives 1.615 s per frame for the old
fire-and-forget stream against 3.83 s per data+ACK cycle, a ratio of
0.57-0.59×. The simulator reproduces the COP-38 cycle (3.83 s) but makes the
old cycle 1.80 s, because its headless PTT release holds about 170 ms longer
per keyed interval than hardware does (see `run_cable_loop`'s doc). The honest
range is therefore **0.57-0.67× of the old clean-link rate**. On a link that
loses frames, the old stream's goodput for a multi-frame B2F message is
effectively zero, since any lost frame corrupts the transfer. Nothing here was
measured on air.

