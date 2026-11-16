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

## Public sources

- The channel names (WGN, MPG, MPP) and the SNR grid (30 to 0 dB in 5 dB
  steps) of the 2020 Winlink comparison of HF modems.
- The Watterson presets of this repository's `coppa-channel`
  (`crates/coppa-channel/src/watterson.rs`): Good 0.5 ms / 0.1 Hz, Poor
  2 ms / 1 Hz.
- This repository's `BENCHMARKS.md` conventions: SNR in a 3 kHz noise
  bandwidth against the clean signal power.

No other modem's source was read and no other modem was run.
