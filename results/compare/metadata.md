# COP-170 comparison data: metadata

```text
coppa_version=0.16.0
seed_base=0x17000000
trials_per_cell=8
platform=linux-x86_64
rustc=1.98.0
jobs=per step, see Commands (rows do not depend on --jobs)
rows=link-l2
recorded_cells=2048 B and 16384 B on wgn, mpg, mpp at 30,25,20,15,10,5,0 dB
```

## Commands

Built once with `cargo build --release -p coppa-bench`, then run in this
order from the repository root. Each run resumes from what is recorded.

1. `target/release/coppa-bench compare --size 2048 --jobs 6 --no-render`
   (stopped after 8 MPP 30 dB trials to reorder the run; those rows stay)
2. `target/release/coppa-bench compare --channel wgn --size 2048 --jobs 6 --no-render` (61 s wall)
3. `target/release/coppa-bench compare --channel mpg --size 2048 --jobs 6 --no-render` (260 s wall)
4. `target/release/coppa-bench compare --channel mpp --size 2048 --jobs 6 --no-render` (457 s wall, 48 trials)
5. `target/release/coppa-bench compare --channel wgn --size 16384 --jobs 32 --no-render --out-dir <scratch>`
   (COP-170 remediation, 21 minutes wall on the same 6-CPU quota, shared with the stopped MPP run and a debug build for most of it). The scratch
   `link-l2.csv` held only these 56 rows. They were appended to
   `results/compare/link-l2.csv`, and
   `target/release/coppa-bench compare --channel wgn --size 16384 --no-render`
   then ran no trial and rewrote the file sorted. Rows do not depend on
   `--jobs` (`compare_link.rs` `jobs_do_not_change_the_csv`). An MPP 16 KB run
   in a different out-dir was stopped unfinished and none of its rows were
   kept.
6. `target/release/coppa-bench compare --channel mpp --size 16384 --no-render`
   (COP-215, jobs 6, 1165 s wall for 56 trials on the 6-CPU quota, shared
   with debug builds and, for its last 5 minutes, with step 7)
7. MPG 16 KB (COP-215), as two concurrent runs in scratch out-dirs so that the
   slow 10 and 5 dB cells start first:
   `target/release/coppa-bench compare --channel mpg --size 16384 --snr 10,5 --jobs 8 --no-render --out-dir <scratch-a>`
   and
   `target/release/coppa-bench compare --channel mpg --size 16384 --snr 30,25,20,15,0 --jobs 6 --no-render --out-dir <scratch-b>`
   (1618 s wall for the 56 trials on the 6-CPU quota). Their rows were appended to
   `results/compare/link-l2.csv`, and
   `target/release/coppa-bench compare --channel mpg --size 16384 --no-render`
   then ran no trial and rewrote the file sorted. A trial's seed comes from
   its cell's place in the full grid, so a run over a subset of SNR points
   records the same rows.
8. `target/release/coppa-bench compare` renders
   `docs/benchmarks/comparison.md` (336 recorded, 0 to run)

Total wall time for the 168 2 KB trials: about 15 minutes on a 6-CPU cgroup quota.
`cargo test --release -p coppa-bench --test compare_results committed_trials_reproduce -- --ignored`
re-ran WGN 0 dB, WGN 30 dB and MPG 20 dB 2 KB trial 0, and MPP 20 dB 16 KB
trial 0 (COP-215), and matched the committed rows byte for byte.

## COP-214 channel cross-check

Built with `cargo build --release -p coppa-bench`, then ran
`target/release/coppa-bench compare channel-check` on Linux x86_64 with
rustc 1.98.0. It wrote `channel-check.csv` at 48 kHz. The three preset rows
use seeds `0x17f00000` through `0x17f00002`. All measured parameters met
their stated tolerances. Run
`target/release/coppa-bench compare --channel wgn --size 16384` to render
the page from the CSV without adding a link trial.

## Black-box self-test

```text
blackbox_version_reply=VERSION Coppa 0.18.0
blackbox_binary_sha256=9ba2347586ba2193b70b6520c84761b26ceb1cd5a0b0e5735c76ac4ff9d3234e
blackbox_coppad_git_sha=d2bcc0a5dba9
blackbox_platform=linux-x86_64
blackbox_payload=seeded
blackbox_rows=6
blackbox_completed=2
```

The Linux black-box command is
`target/release/coppa-bench compare blackbox --modem tools/compare/coppad.toml`.
Build it with `cargo build --release -p coppa-bench --features cpal-backend`
and build the daemon with
`cargo build --release -p coppa-daemon --bin coppad --features cpal-backend,ardop,kiss-tnc`.
The command defaults to a single WGN 30 dB 2 KB smoke trial and writes under
`target/compare-foreign/coppad/` unless `--out-dir` is set.

COP-227 re-recorded the six published rows on Linux x86_64 under a six-CPU
cgroup quota (`/sys/fs/cgroup/cpu.max` read `600000 100000`). The daemon was
built once with
`COPPA_GIT_SHA=d2bcc0a5dba9 cargo build --release --locked -p coppa-daemon --bin coppad --features cpal-backend,ardop,kiss-tnc`
and the harness with
`cargo build --release --locked -p coppa-bench --features cpal-backend`.
The two cells then ran at the same time, each in its own fresh out-dir:
`target/release/coppa-bench compare blackbox --modem tools/compare/coppad.toml --channel wgn --snr 30 --size 2048 --trials 3 --jobs 1 --out-dir target/compare-rerecord/wgn`
and the same command with `--channel mpg --snr 20` and
`--out-dir target/compare-rerecord/mpg`. Their rows were joined under one
header into `blackbox-coppad.csv`. Both integrity probes printed
`correlation 1.000000, level +0.00 dB, cable lead 355 ms: pass`, and both
calibrations measured active TX reference powers of 0.023498 and 0.020689.
WGN 30 dB: trials 0 and 1 completed, 2048 bytes in 58.5 s and 59.8 s;
trial 2 capped with 525 bytes delivered. MPG 20 dB: trials 0, 1 and 2
capped with 518, 526 and 1620 bytes delivered. Two of the six trials
completed (2/3 WGN, 0/3 MPG). Every trial counted 0 starved and 0 overflow
ticks. For about four minutes of MPG trial 1, after the WGN cell had
finished, a debug build and clippy ran beside it at `nice -n 19` with
`-j 2`; that trial capped at 526 bytes, as the same seed did at planning
with nothing beside it. Each capped trial connected and had delivered 518 to 1620 payload
bytes when the 600 s cap ended it. They look like the stall described in
the follow-up ticket COP-237; this is an observation, not a diagnosis.

Each row records `payload_crc32`, the CRC-32 of the seeded bytes the trial
wrote, and `compare blackbox` refuses to resume into a CSV without that
column or whose row carries another value for its seed and size. Before
COP-227 the published rows came from coppad 0.16.0 (sha256 `9b8b4957…`) and
wrote a 256-byte repeating ramp. None of those rows remains. The ramp had
not lowered their on-air work. coppad compresses each frame's chunk on its
own, and no frame holds more than 1424 encoded bytes (eight codewords at
level 4 or 7). With HF_STANDARD compression, the 2048-byte ramp encodes to
3550 bytes and the six seeds' payloads to 3509 to 3583, so up to that size
the ramp compresses no better than seeded bytes. The ramp only compresses
better at larger sizes (3572 or 3573 bytes at 4096, against 7024 to 7112 for
the same seeds), and no frame reaches them. COP-226 ran coppad's chunker on
the ramp and on seeded bytes at levels 2, 4 and 8, for 2048 and 16384 bytes,
and the frame count and airtime matched in every case.

To extend these cells, resume with the binary whose sha256 is
`blackbox_binary_sha256`. A release build's sha256 changes with the checkout
path and `COPPA_GIT_SHA`, so a rebuild elsewhere cannot resume these rows;
re-record all six with it instead.

## Public sources

- The channel names (WGN, MPG, MPP) and the SNR grid (30 to 0 dB in 5 dB
  steps) of the 2020 Winlink comparison of HF modems.
- The Watterson presets of this repository's `coppa-channel`
  (`crates/coppa-channel/src/watterson.rs`): Good 0.5 ms / 0.1 Hz, Poor
  2 ms / 1 Hz.
- This repository's `BENCHMARKS.md` conventions: SNR in a 3 kHz noise
  bandwidth against the clean signal power.

No other modem's source was read and no other modem was run.
