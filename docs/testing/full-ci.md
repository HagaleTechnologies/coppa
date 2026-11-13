# Full CI test groups

COP-200 separates the full suite into four independently scheduled jobs.
The `ci` Cargo profile inherits `test`, uses optimization level 2 and line
tables, and explicitly retains debug assertions and overflow checks.
Incremental compilation is disabled. Local `cargo test --workspace` and the
release profile retain their prior settings.

Run a group with:

```sh
COPPA_REQUIRE_BROWSER=1 RUST_TEST_THREADS=2 bash tools/ci/run-full-tests.sh workspace
COPPA_REQUIRE_BROWSER=1 RUST_TEST_THREADS=2 bash tools/ci/run-full-tests.sh daemon-unit
COPPA_REQUIRE_BROWSER=1 RUST_TEST_THREADS=2 bash tools/ci/run-full-tests.sh daemon-integration
COPPA_REQUIRE_BROWSER=1 RUST_TEST_THREADS=2 bash tools/ci/run-full-tests.sh no-audio
```

All commands use `--locked --profile ci`. The runner records compile,
execution and doctest wall times, including nonzero exits. An error stops
the group with Cargo's exit code. The matrix disables fail-fast, and the
required `CI` fan-in requires the matrix aggregate to succeed. Its queue
watchdog names every matrix job. Dispatch checkout attribution and the
fast-tier polling logic are unchanged.

## Coverage boundaries

The workspace group excludes only the daemon. It runs library, binary and
integration targets, compiles examples separately, then runs doctests.
The daemon unit group runs library and binary targets. The daemon
integration group runs all integration targets, doctests and example
compilation. No group executes benchmarks.

The full feature set is `cpal-backend,websocket,ardop,mdns,kiss-tnc`.
The workspace group omits only `mdns`: Cargo rejects that feature when
the daemon, its sole defining package, is excluded. Both daemon groups
retain it.

The no-audio commands retain their original selections:

```sh
cargo test --locked --profile ci -p coppa-cli --features kiss-tnc --test tnc_agw
cargo test --locked --profile ci -p coppa-daemon --features websocket,mdns,kiss-tnc --test cli --test ptt_release --test reload --test runtime_smoke
```

These cover process tests compiled out with CPAL. They run in separate
Cargo invocations so full-feature selection cannot unify audio into them.
Existing ignored/manual tests remain ignored. No test assertions, payloads,
seeds, retry budgets or clocks were changed. The production VARA command
server now enables TCP_NODELAY for command replies, as described in the
local execution record below.

`tests/full_ci_contract.rs` checks runner arguments against Cargo target
metadata and rejects an intentionally incomplete selection. It also
executes the profile's debug checks, injects errors into every command
position, executes the actual fan-in shell, and exercises the actual
watchdog against queued, running and completed job fixtures.

## Measurement environment

Implementation measurements use rustc 1.98.0 on Linux x86_64, with
`/sys/fs/cgroup/cpu.max = 600000 100000` (six CPU quota cores) and
`memory.max = 9663676416` (9 GiB). Chromium is available at
`/usr/local/bin/chromium`, and ALSA development headers are installed.
Measurements set `CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=2`.
Dependency downloads were cached; each group's first target directory was
empty. Initial runs were isolated; later runs overlapped to fit the phase window.
The detailed record below distinguishes these conditions. The workspace warm
repeat reused its corresponding target directory.

The dropped-file-chunk test uses `start_paused = true`. Its 29.5/30-second
Tokio sleeps advance virtual time. The unpaused event-loop RTO helper
instead sleeps on the real thread. Optimizing DSP computation does not
remove those real waits.

## Hosted confirmation

Local measurements do not prove an end-to-end hosted verdict below 30
minutes. For one unchanged revision, collect three hosted Ubuntu full
workflow runs, including a build-cold run and warm repeats, with fast CI
already green. Require each workflow-start-to-required-`CI` conclusion
below 1800 seconds and no test/job timeouts. Report queue time separately
and include other required jobs in the critical path. Hosted confirmation
is pending.

## Local execution record

Measured on 2026-11-13 in the COP-200 working tree. Each cold run started
with an empty target directory and cached downloads.

| Group | Cold total seconds | Warm total seconds | Cold passed / ignored |
|---|---:|---:|---:|
| workspace | 746.06 | 264.8 | 2006 / 18 |
| daemon-unit | 1015.99 | not run | 1420 / 5 |
| daemon-integration | 259.42 | not run | 79 / 0 |
| no-audio | 290.93 | not run | 87 / 0 |

All four cold totals are below 1500 seconds. The workspace cold and warm
runs were isolated. Overlap began at 07:54 UTC, during the tail of daemon-unit
execution. Remaining cold groups ran alongside inventory compilation,
exact-name repetitions and lint/build checks. These are successful bounds
under additional load, not a controlled speedup comparison. Each Cargo
invocation used two build jobs; simultaneous invocations shared six quota
cores. Only the workspace warm repeat was completed. Other warm repeats
were omitted to reserve the mandatory final default-profile gate within
the phase window. No workload is omitted from the actual CI runner.

Initial setup attempts failed before tests because Cargo rejects mdns when
its owning daemon package is excluded, and because adding the JSON
dev-dependency required the root lockfile entry to change. Those attempts
are excluded here; successful cold runs used new target directories.
No failing simulation was retried to obtain the 24 named-repeat results.
The first integration run did fail four BPQ startup tests at line 49.
Pinned baseline sources built with the proposed profile also failed all four
in 0.68 seconds. Client-side TCP_NODELAY did not help and was reverted.
Disabling Nagle buffering on server command replies made all four pass in
5.01 seconds, without changing deadlines or assertions. The integration
table row is a new build-cold full-group execution after that fix. Other
group timings and named repetitions precede this socket-policy change.
No existing Rust test source was edited. This is not a cross-revision
speedup comparison.

## Before/after test inventory

The baseline is fa4de8eb0fa18d8613180b11141c3914e5310ffb in a temporary
worktree. Its only modification was the matching ci profile for compilation.
The three original commands were run with --locked --profile ci and
-- --list. New selections used the same settings, with library/binary,
integration and doctest lists collected separately. Normalization retained
feature configuration, target executable and exact test name. The new
full_ci_contract target is excluded from this comparison of existing tests.

Inventory: 3607 identities before, 3607 after, 0 missing, 0 added existing identities.

- full: 3520 before, 3520 after.
- no-audio-cli: 4 before, 4 after.
- no-audio-daemon: 83 before, 83 after.

Doctests have a separate doc target prefix. Cargo metadata provides the
package/target-kind inventory in the executable contract test, whose
negative control removes daemon integration selection. No existing Rust
test source was edited, so ignored annotations are unchanged. Successful
execution counts above retain their ignores.

Baseline inventory commands:

    cargo test --locked --profile ci --workspace --features cpal-backend,websocket,ardop,mdns,kiss-tnc -- --list
    cargo test --locked --profile ci -p coppa-cli --features kiss-tnc --test tnc_agw -- --list
    cargo test --locked --profile ci -p coppa-daemon --features websocket,mdns,kiss-tnc --test cli --test ptt_release --test reload --test runtime_smoke -- --list

After commands are the runner selections with -- --list appended,
including both separate --doc selections. Example compilation is verified
by the successful cold groups and Cargo metadata.

## Exact-name simulation repeats

The actual binaries from the measured ci-profile builds supplied --list.
Each invocation used BINARY --exact FULL_NAME --nocapture and had to report
exactly 1 passed; 0 failed. Each test ran three consecutive times without
retry. Different names ran concurrently; each name kept its own three-run
sequence. All seven daemon cases used all five CI features. The engine
defines no feature flags. Times include shared load.

- dropped_file_chunk_is_repaired_by_next_copy_without_return_channel: 7.19s, 7.24s, 7.39s; all passed.
- engine::tests::encoded_payload_len_predicts_encode_bytes_acceptance: 8.52s, 8.80s, 8.68s; all passed.
- event_loop::tests::data_delivery::session_data_survives_a_host_that_is_not_reading: 14.21s, 13.38s, 11.95s; all passed.
- event_loop::tests::impaired_hf_session_delivers_data_and_audio_ack: 10.63s, 10.88s, 10.63s; all passed.
- event_loop::tests::session_write_of_8000_bytes_is_chunked_acked_and_reassembled: 10.94s, 11.26s, 10.89s; all passed.
- event_loop::tests::telemetry::a_retransmission_that_no_longer_encodes_still_spends_the_budget: 202.71s, 202.35s, 202.31s; all passed.
- event_loop::tests::telemetry::fade_hold_fallback_accepts_multi_codeword_segment: 158.64s, 158.04s, 157.91s; all passed.
- event_loop::tests::telemetry::large_raw_write_chunks_into_modem_sized_frames: 9.23s, 9.77s, 9.50s; all passed.

These 24 successes establish the requested repeat check, not a zero flake
rate. RTO-heavy cases still wait in real time; optimizing DSP preserves
their clock semantics.

## Tool checks

The contract suite passed in the optimized workspace run. Shellcheck
accepts the runner. Actionlint 1.7.12 with shellcheck integration disabled
accepts the workflow; pre-existing workflow shellcheck warnings are
unchanged. The offline wiki manifest tests pass. Private sunto wiki-lint
is unavailable; the local guard verifies page fields and body hashes.
The final default-profile workspace gate is reported by the phase.
