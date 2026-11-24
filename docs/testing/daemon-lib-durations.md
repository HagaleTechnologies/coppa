# Slowest coppa-daemon lib tests (COP-239)

This record names the slowest tests in the `coppa-daemon` lib test binary and
their wall times, before and after COP-239. COP-224's implement phase ran out
of its 4800 s gate cap inside this binary with no test failing: in a debug
build it took 1420 s on 32 test threads and 3820 s on 2. COP-239 makes the
binary, and so `cargo test --workspace`, fit that gate.

## How to measure

```sh
cargo test -p coppa-daemon --lib --no-run      # prints the binary's path
cp target/debug/deps/coppa_daemon-<hash> <scratch>/daemon-lib-bin
cd crates/coppa-daemon
RUSTC_BOOTSTRAP=1 <scratch>/daemon-lib-bin -Z unstable-options --report-time > <scratch>/after.log 2>&1
grep "test result" <scratch>/after.log
sed -nE 's/^test ([^ ]+) \.\.\. ok <([0-9.]+)s>/\2 \1/p' <scratch>/after.log | sort -rn | head -20
sed -nE 's/^test ([^ ]+) \.\.\. ok <([0-9.]+)s>/\2/p' <scratch>/after.log | awk '{s+=$1} END {print s}'
```

- `--report-time` is a nightly libtest flag. `RUSTC_BOOTSTRAP=1` at run time
  makes the stable libtest accept it, with no toolchain change. Each line then
  reads `test <name> ... ok <12.345s>`.
- Run from `crates/coppa-daemon`, as `cargo test` does, so that tests reading
  relative paths find their files.
- Copy the binary first, so that a later build cannot replace it mid-run, and
  delete the copy afterwards.
- The default test-thread count follows the cgroup CPU quota
  (`/sys/fs/cgroup/cpu.max`), not `nproc`. On a 2-CPU quota libtest runs two
  threads even when `nproc` prints 32.

## Environment

| | before (research) | after (implement) |
|---|---|---|
| tree | base `9498632` | the COP-239 branch |
| rustc | 1.98.0 | 1.98.0 |
| `cpu.max` | `200000 100000` (2 CPUs) | `600000 100000` (6 CPUs) |
| `nproc` | 32 | 32 |
| test threads | 2 | 6 |
| features | default | default |
| profile | `test`, all crates at opt-level 0 | `test` with the COP-239 package overrides |

The two runs had different CPU quotas, so compare the per-test sums, not the
`finished in` lines. The planning phase also ran the COP-239 change on a 2-CPU
quota; its figures are under "What changed".

## Before

Base `9498632`, debug, 2 threads: `test result: ok. 622 passed; 0 failed;
3 ignored; 0 measured; 0 filtered out; finished in 3819.89s`. The sum of the
622 per-test times is 7635 s. "Paused" tests run on
`#[tokio::test(start_paused = true)]`; "live" tests run on the real clock.

| s | kind | test |
|---:|---|---|
| 1494.3 | paused | `event_loop::tests::floor_connect::cop222_oversized_segment_delivered_on_floor` |
| 794.4 | paused | `event_loop::tests::floor_connect::cop222_outer_segment_delivered_on_floor` |
| 685.8 | paused | `event_loop::tests::floor_connect::floor_daemon_pair_connects_and_delivers` |
| 465.5 | paused | `event_loop::tests::floor_connect::floor_daemon_five_ack_probe` |
| 317.8 | paused | `event_loop::tests::floor_connect::floor_daemon_simultaneous_calls` |
| 298.6 | paused | `event_loop::tests::cop60_turnaround::rs_cable_write_2000_bytes_arrives_intact` |
| 216.7 | paused | `event_loop::tests::cop60_turnaround::rs_cable_lost_double_ack_reacks_without_duplicate_delivery` |
| 211.1 | paused | `event_loop::tests::floor_connect::floor_daemon_phy_mode_reporting` |
| 204.6 | live, real sleeps | `event_loop::tests::telemetry::a_retransmission_that_no_longer_encodes_still_spends_the_budget` |
| 174.9 | paused | `event_loop::tests::cop60_turnaround::rs_cable_retains_packets_across_attempts` |
| 167.0 | live, real sleeps | `event_loop::tests::telemetry::fade_hold_fallback_accepts_multi_codeword_segment` |
| 147.0 | paused | `event_loop::tests::cop60_turnaround::cable_loop_session_write_of_1000_bytes_arrives_intact_in_order` |
| 114.5 | paused | `event_loop::tests::floor_connect::floor_daemon_duplicate_handshake` |
| 93.1 | paused | `event_loop::tests::cop60_turnaround::rs_cable_six_erased_packets_need_no_retransmission` |
| 83.9 | live | `event_loop::tests::data_delivery::session_data_survives_a_host_that_is_not_reading` |
| 66.9 | paused | `event_loop::tests::cop60_turnaround::cable_loop_lost_session_frame_is_retransmitted_by_the_run_loop` |
| 66.2 | paused | `event_loop::tests::cop60_turnaround::cable_loop_lost_session_ack_is_recovered_without_a_duplicate_delivery` |
| 64.5 | live | `event_loop::tests::session_write_of_8000_bytes_is_chunked_acked_and_reassembled` |
| 64.1 | live | `event_loop::tests::telemetry::large_raw_write_chunks_into_modem_sized_frames` |
| 56.5 | live | `event_loop::tests::telemetry::large_arq_write_fills_window_then_pumps_on_ack` |

The host is shared. One test took 83.9 s in this run and 115.0 s when rerun
alone, so read any single figure as plus or minus about 35%.

## After

The COP-239 branch, 6 threads: `test result: ok. 622 passed; 0 failed;
3 ignored; 0 measured; 0 filtered out; finished in 109.77s`. The sum of the
622 per-test times is 560.9 s, against 7635 s before.

| s | test |
|---:|---|
| 79.9 | `event_loop::tests::floor_connect::cop222_oversized_segment_delivered_on_floor` |
| 42.8 | `event_loop::tests::floor_connect::cop222_outer_segment_delivered_on_floor` |
| 36.4 | `event_loop::tests::floor_connect::floor_daemon_pair_connects_and_delivers` |
| 24.5 | `event_loop::tests::floor_connect::floor_daemon_five_ack_probe` |
| 24.2 | `event_loop::tests::cop60_turnaround::rs_cable_write_2000_bytes_arrives_intact` |
| 18.8 | `event_loop::tests::floor_connect::floor_daemon_simultaneous_calls` |
| 13.1 | `event_loop::tests::cop60_turnaround::rs_cable_lost_double_ack_reacks_without_duplicate_delivery` |
| 11.5 | `event_loop::tests::floor_connect::floor_daemon_phy_mode_reporting` |
| 11.0 | `event_loop::tests::cop60_turnaround::rs_cable_retains_packets_across_attempts` |
| 8.1 | `event_loop::tests::floor_connect::floor_daemon_duplicate_handshake` |
| 7.4 | `event_loop::tests::cop60_turnaround::cable_loop_session_write_of_1000_bytes_arrives_intact_in_order` |
| 6.9 | `event_loop::tests::cop60_turnaround::rs_cable_six_erased_packets_need_no_retransmission` |
| 6.6 | `event_loop::tests::impaired_hf_session_delivers_data_and_audio_ack` |
| 6.1 | `event_loop::tests::telemetry::large_arq_write_fills_window_then_pumps_on_ack` |
| 5.9 | `event_loop::tests::data_delivery::session_data_survives_a_host_that_is_not_reading` |
| 5.4 | `event_loop::tests::session_write_of_8000_bytes_is_chunked_acked_and_reassembled` |
| 4.7 | `event_loop::tests::after_a_failed_negotiation_a_later_negotiation_still_succeeds` |
| 4.5 | `event_loop::tests::telemetry::large_raw_write_chunks_into_modem_sized_frames` |
| 4.1 | `event_loop::tests::cop60_turnaround::cable_loop_lost_session_ack_is_recovered_without_a_duplicate_delivery` |
| 4.1 | `event_loop::tests::a_rate_loop_vhf_excursion_does_not_desync_a_negotiated_short_cp` |

The two telemetry tests that waited on real time now take 0.28 s
(`a_retransmission_that_no_longer_encodes_still_spends_the_budget`) and
0.80 s (`fade_hold_fallback_accepts_multi_codeword_segment`) when run alone.

Cold `cargo test --workspace` on the same tree and quota, starting from an
empty target directory with downloads cached, passed in 726 s: 218 s for
`cargo test --workspace --no-run`, then 508 s for `cargo test --workspace`.
It printed 95 `test result: ok` lines (3337 passed, 0 failed, 27 ignored).
The slowest binaries were:

| s | passed | binary |
|---:|---:|---|
| 105.87 | 622 | `coppa_daemon` lib unit tests |
| 104.13 | 633 | `coppad` bin unit tests |
| 81.90 | 7 | `tests/proptest_roundtrip.rs` |
| 43.13 | 257 | `coppa_bench` lib unit tests |
| 30.31 | 1 | `coppa-bench` `tests/compare_channel.rs` |

The target directory then measured 13 GB (`du -sh target`). In planning, on
a 2-CPU quota, the same cold run took 1551 s (493 s plus 1058 s).

## What changed

- **Optimized dependencies.** `cargo test` now builds every non-workspace
  crate and `coppa-dsp`, `coppa-codec`, `coppa-protocol`, `coppa-channel`,
  `coppa-ml` and `coppa-engine` at opt-level 2 (`[profile.test.package...]`
  in the root `Cargo.toml`). Only opt-level changes, so debug assertions and
  overflow checks stay on. `coppa-daemon` and the other crates stay at
  opt-level 0, so editing the daemon's tests recompiles only that crate.
  This is the lever for the paused-clock simulations at the top of both
  tables. They already cost no wall time while they wait; their time is
  synchronous DSP work, which a paused clock cannot shorten. With this lever
  alone, the first planning attempt measured `finished in 464.37s` on a 2-CPU
  quota.
- **Paused clock for RTO waits.** 15 default-feature tests let a session
  RTO pass with `std::thread::sleep`. Fourteen of them moved from a live clock
  to `#[tokio::test(start_paused = true)]`; the fifteenth,
  `cop222_oversized_retry_requires_negotiation`, was already paused. All 15
  now call `advance_past_session_rto`, which moves tokio's clock past the
  RTO. Their assertions did not change.
  This removed the 202 s and 158 s of real waiting in the two telemetry
  tests. The RTO doubles per timeout, so each pass slept longer than the
  last: a trace of one showed sleeps of 3.1 s, 6.3 s, 12.5 s and 25.0 s.
  Four test clock reads changed from `Instant::now()` to `clock_now()`,
  because they compare against or seed daemon times that follow tokio's
  clock.
- **Both together**, planning phase, 2-CPU quota: `finished in 301.99s`
  with a per-test sum of 602.8 s, against 3819.89 s and 7635 s at base.

## What is left

- The slowest tests are CPU-bound paired-audio simulations
  (`floor_connect`, `cop60_turnaround`) that already run on a paused clock.
  On many cores the binary's wall time is its slowest test, now about 80 s.
  Shortening them would mean changing what they assert, such as the COP-222
  300-byte payload.
- The converted tests still need the RTO to pass. With
  `advance_past_session_rto` mutated to advance `Duration::ZERO` (a local
  run, never committed), 14 of the 15 failed: two on `take_leg`'s
  "expected the station to have really transmitted the retransmission", six
  on `exhaust_into_fade_hold`'s "the fade hold must be entered within ...
  retransmit passes", and six on their own assertions.
- The fifteenth, `cop222_oversized_retry_requires_negotiation`, passed under
  the mutation. Its final assertions (no fragment and an OFDM `phy_tx`) hold
  whether or not the retransmission happens. Before COP-239 it was already
  paused and its real sleep never moved tokio's clock, so it never exercised
  a retransmission. It now does, but no assertion checks it.
- The websocket chat test `a_retransmitted_chat_segment_is_byte_identical`
  still sleeps one RTO on the real thread. It drives real sockets, so it
  stays unpaused.
- `coppad`'s unit-test binary re-declares the daemon's modules, so
  `cargo test --workspace` runs about 616 of these tests twice.
- Disk. In planning, a full local gate in one target directory reached
  13 GB after `cargo test --workspace` and 20 GB by `du -sh` after the rest
  of the COP-224 gate. Running `cargo build` and `cargo test` in one target
  now compiles dependencies twice, once per profile setting.

See [LIMITATIONS](../LIMITATIONS.md) for the open items.
