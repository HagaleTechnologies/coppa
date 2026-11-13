> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-24: 64-QAM 5/6 dropped from HF; ladder renumbered 1-9

Measured 2026-10-23 on the COP-24 implementation. Version 0.2.0 renumbers
old wire level 9 to 8 and old level 10 to 9. HF engines carry levels 1-8;
level 9 remains available on VHF. Earlier sections below retain their
pre-COP-24 numbering. See `docs/adr/009-cop24-speed-ladder-renumber.md`.

The 300-2700 Hz SSB filter attenuates the 2650 Hz carrier by 2.54 dB and
the 2700 Hz carrier by 6.02 dB; the highest payload pilot is at 2450 Hz.
Those measured response values support the band-edge explanation, but do
not isolate estimation error from insufficient coding margin.

The fresh sweep deliberately bypasses the engine's HF ceiling through
`CoppaTransceiver`, forcing every level onto `hf_standard`, including the
VHF-only level 9, to retain a failing comparison:

```sh
cargo build --release -p coppa-bench
./target/release/coppa-bench --trials 50 --snr-min 18 --snr-max 60 --snr-step 6 --channel awgn --profile standard --ssb --seed 2410
```

FER, 50 trials per cell, through the SSB filter:

| Wire level | 18 dB | 24 dB | 30 dB | 36 dB | 42 dB | 48 dB | 54 dB | 60 dB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 0% | 0% | 0% | 0% | 0% | 0% | 0% | 0% |
| 2 | 0% | 0% | 0% | 0% | 0% | 0% | 0% | 0% |
| 3 | 0% | 0% | 0% | 0% | 0% | 0% | 0% | 0% |
| 4 | 0% | 0% | 0% | 0% | 0% | 0% | 0% | 0% |
| 5 | 0% | 0% | 0% | 0% | 0% | 0% | 0% | 0% |
| 6 | 0% | 0% | 0% | 0% | 0% | 0% | 0% | 0% |
| 7 | 0% | 0% | 0% | 0% | 0% | 0% | 0% | 0% |
| 8 | 0% | 0% | 0% | 0% | 0% | 0% | 0% | 0% |
| 9 | 100% | 98% | 96% | 92% | 96% | 100% | 100% | 94% |

Every shipped HF level had 0/50 errors at every tested SNR from 18 to 60 dB.
Level 9 never cleared 10% FER; every failure was LDPC non-convergence.

The engine-path acceptance test is
`crates/coppa-engine/tests/hf_ssb_ladder.rs::every_shipped_hf_level_decodes_through_ssb`.
It iterates `CoppaCore::from_profile(&HF_STANDARD).available_speed_levels()`,
encodes 24 seeded random bytes, applies `ssb_filter` and `awgn_seeded` at
30 dB, and decodes through a fresh HF_STANDARD engine. It runs 10 trials
per level and permits at most one frame error per level. Seeds are
`24100 + level * 100 + trial`, with `StdRng` generating payload bytes.

| Wire level | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Engine frame errors / 10 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

The negative control temporarily changed `HF_MAX_SPEED_LEVEL` from 8 to
9 and reran the same acceptance test. Level 9 had **4/10 frame errors**,
all LDPC non-convergence, while levels 1-8 remained at 0/10. The test
failed with `HF level 9: FER 4/10 exceeds 10%`. The ceiling was restored
to 8. This failure demonstrates that the test detects reintroducing the
mode on HF; it does not claim the same FER as a full-payload PHY sweep.
The payload lengths, seeds and engine compression path differ.

These are in-process filter/channel measurements. No live two-radio
field test was performed.

