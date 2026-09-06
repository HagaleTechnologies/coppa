# Fresh sweeps from the 2026-09-05 product review (dimension F)

All runs: `coppa-bench` at `1c2ffc5`, `--profile standard` (i.e. `hf_standard`
forced at **every** speed level, bypassing the engine's default routing of
levels >= 5 to `vhf_wide`), 3 kHz-referenced clean-signal SNR, seed 12648430.
FER/goodput are deterministic; wall-clock was not recorded (shared host).

| Dir | Channel | SSB filter | SNR sweep | Trials/pt |
|---|---|---|---|---|
| `awgn-ssb-std` | AWGN | 300-2700 Hz | 0..27 dB step 3 | 50 |
| `awgn-ssb-std-hi` | AWGN | 300-2700 Hz | 30..60 dB | 10 |
| `awgn-nossb-std-hi` | AWGN | none | 30..60 dB | 10 |
| `poor-ssb-std` | Watterson CCIR Poor (2 ms / 1 Hz) | 300-2700 Hz | 6..30 dB step 6 | 50 |
| `mod-ssb-std` | Watterson CCIR Moderate (1 ms / 0.5 Hz) | 300-2700 Hz | 6..30 dB step 6 | 50 |

Headline: level 10 (64-QAM 5/6) on `hf_standard` is undecodable through the
SSB filter at every SNR up to 60 dB (`ldpc_not_converged`), while decoding
cleanly unfiltered. See `docs/reviews/2026-09-05-dim-F-modem-fitness.md`.
