# COP-170 comparison data: metadata

```text
coppa_version=0.16.0
seed_base=0x17000000
trials_per_cell=8
platform=linux-x86_64
rustc=1.98.0
jobs=6
rows=link-l2
recorded_cells=2048 B on wgn, mpg, mpp at 30,25,20,15,10,5,0 dB; 16384 B on wgn at 30,25,20,15,10,5,0 dB
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
6. `target/release/coppa-bench compare --channel wgn --size 16384` renders
   `docs/benchmarks/comparison.md` (no trial runs)

Total wall time for the 168 2 KB trials: about 15 minutes on a 6-CPU cgroup quota.
`cargo test --release -p coppa-bench --test compare_results committed_trials_reproduce -- --ignored`
re-ran WGN 0 dB, WGN 30 dB and MPG 20 dB trial 0 and matched the committed
rows byte for byte.

The MPG and MPP 16 KB cells are not recorded. Record them with
`cargo run --release -p coppa-bench -- compare --size 16384 --no-render`, then
render with `cargo run --release -p coppa-bench -- compare`.

## COP-214 channel cross-check

Built with `cargo build --release -p coppa-bench`, then ran
`target/release/coppa-bench compare channel-check` on Linux x86_64 with
rustc 1.98.0. It wrote `channel-check.csv` at 48 kHz. The three preset rows
use seeds `0x17f00000` through `0x17f00002`. All measured parameters met
their stated tolerances. Run
`target/release/coppa-bench compare --channel wgn --size 16384` to render
the page from the CSV without adding a link trial.

The Linux black-box command is
`target/release/coppa-bench compare blackbox --modem tools/compare/coppad.toml`.
Build it with `cargo build --release -p coppa-bench --features cpal-backend`
and build the daemon with
`cargo build --release -p coppa-daemon --bin coppad --features cpal-backend,ardop,kiss-tnc`.
The command defaults to a single WGN 30 dB 2 KB smoke trial and writes under
`target/compare-foreign/coppad/` unless `--out-dir` is set.

The published coppad self-test was recorded with release builds on Linux
x86_64 using the two-CPU container quota. Both runs used `--trials 3` and
`--jobs 3`. WGN 30 dB, 2048 bytes wrote to `results/compare`; MPG 20 dB,
2048 bytes wrote to a separate scratch directory so its lines could be checked
before appending them to `blackbox-coppad.csv`. The 10 s integrity probe
measured correlation 1.000000, level error +0.00 dB and 355 ms lead; it
counted no starved or overflow tick. A noiseless 40-byte link exchange in
both directions measured active TX reference powers of 0.022691 and
0.020689. All six trial rows completed exactly. Parallel scheduling affected
the WGN throughput, so these are real-time self-test numbers, not
bit-reproducible link rows.

## Public sources

- The channel names (WGN, MPG, MPP) and the SNR grid (30 to 0 dB in 5 dB
  steps) of the 2020 Winlink comparison of HF modems.
- The Watterson presets of this repository's `coppa-channel`
  (`crates/coppa-channel/src/watterson.rs`): Good 0.5 ms / 0.1 Hz, Poor
  2 ms / 1 Hz.
- This repository's `BENCHMARKS.md` conventions: SNR in a 3 kHz noise
  bandwidth against the clean signal power.

No other modem's source was read and no other modem was run.
