# COP-170 comparison data: metadata

```text
coppa_version=0.16.0
seed_base=0x17000000
trials_per_cell=8
platform=linux-x86_64
rustc=1.98.0
jobs=6
rows=link-l2
recorded_cells=2048 B on wgn, mpg, mpp at 30,25,20,15,10,5,0 dB
```

## Commands

Built once with `cargo build --release -p coppa-bench`, then run in this
order from the repository root. Each run resumes from what is recorded.

1. `target/release/coppa-bench compare --size 2048 --jobs 6 --no-render`
   (stopped after 8 MPP 30 dB trials to reorder the run; those rows stay)
2. `target/release/coppa-bench compare --channel wgn --size 2048 --jobs 6 --no-render` (61 s wall)
3. `target/release/coppa-bench compare --channel mpg --size 2048 --jobs 6 --no-render` (260 s wall)
4. `target/release/coppa-bench compare --channel mpp --size 2048 --jobs 6 --no-render` (457 s wall, 48 trials)
5. `target/release/coppa-bench compare --size 2048` renders `docs/benchmarks/comparison.md` (no trial runs)

Total wall time for the 168 trials: about 15 minutes on a 6-CPU cgroup quota.
`cargo test --release -p coppa-bench --test compare_results committed_trials_reproduce -- --ignored`
re-ran WGN 0 dB, WGN 30 dB and MPG 20 dB trial 0 and matched the committed
rows byte for byte.

The 16 KB cells are not recorded. Record them with
`cargo run --release -p coppa-bench -- compare --size 16384 --no-render`, then
render with `cargo run --release -p coppa-bench -- compare`.

## Public sources

- The channel names (WGN, MPG, MPP) and the SNR grid (30 to 0 dB in 5 dB
  steps) of the 2020 Winlink comparison of HF modems.
- The Watterson presets of this repository's `coppa-channel`
  (`crates/coppa-channel/src/watterson.rs`): Good 0.5 ms / 0.1 Hz, Poor
  2 ms / 1 Hz.
- This repository's `BENCHMARKS.md` conventions: SNR in a 3 kHz noise
  bandwidth against the clean signal power.

No other modem's source was read and no other modem was run.
