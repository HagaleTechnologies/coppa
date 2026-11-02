> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## Sample-clock offset over session-length transmissions (COP-3)

Measured at revision `8f11bda` with:

```bash
cargo run -p coppa-bench --release --example sample_clock_offset
```

The diagnostic used level 2 with the HF-standard profile and the signed relative
receiver-clock convention `scale = 1 + ppm / 1e6` (positive ppm shortens the
receiver buffer). Frame cells used 20 paired deterministic trials, clean-signal-
referenced AWGN, and fixed operating points: AWGN/30 dB, Watterson Good/18 dB,
Moderate/24 dB, and Poor/30 dB. The session cells used one paired deterministic
600-second ARQ session per `(preset, ppm)` with the existing 20→0→20 dB ramp.

### Frame results

| Channel | ppm −120 | −100 | −50 | 0 | +50 | +100 | +120 |
|---|---:|---:|---:|---:|---:|---:|---:|
| AWGN 30 dB | 0/20 | 0/20 | 0/20 | 0/20 | 0/20 | 0/20 | 0/20 |
| Watterson Good 18 dB | 3/20 | 1/20 | 1/20 | 1/20 | 2/20 | 2/20 | 2/20 |
| Watterson Moderate 24 dB | 2/20 | 2/20 | 2/20 | 2/20 | 2/20 | 2/20 | 2/20 |
| Watterson Poor 30 dB | 11/20 | 9/20 | 9/20 | 9/20 | 9/20 | 10/20 | 10/20 |

Entries are frame errors/trials. All AWGN cells had FER 0.000 (95% Wilson
interval 0.000–0.161). The Good, Moderate, and Poor zero-ppm FER values were
0.050 (0.009–0.236), 0.100 (0.028–0.301), and 0.450 (0.258–0.658). Every
signed-offset result overlaps its corresponding zero-ppm interval. Across all
560 frame trials there were no wrong payloads, header-corrupt outcomes, or CRC
mismatches. Poor had one sync failure in every cell; all remaining failures were
LDPC non-convergence. Thus the tested SCO does not introduce a new sync/header
failure mode.

### Ten-minute ARQ session results

| Preset | ppm | Completed/dropped | Retransmits | Delivered bytes | bytes/min |
|---|---:|---:|---:|---:|---:|
| Good | −120 | 1/0 | 31 | 16,380 | 1,634.4 |
| Good | −100 | 0/1 | 19 | 7,605 | 1,111.3 |
| Good | −50 | 1/0 | 25 | 16,965 | 1,690.9 |
| Good | 0 | 0/1 | 9 | 7,605 | 1,114.4 |
| Good | +50 | 0/1 | 9 | 7,605 | 1,114.4 |
| Good | +100 | 1/0 | 22 | 17,082 | 1,702.8 |
| Good | +120 | 0/1 | 12 | 7,605 | 1,103.4 |
| Moderate | −120 | 0/1 | 15 | 7,722 | 1,131.0 |
| Moderate | −100 | 0/1 | 15 | 7,722 | 1,131.0 |
| Moderate | −50 | 0/1 | 16 | 7,254 | 991.6 |
| Moderate | 0 | 1/0 | 32 | 14,976 | 1,491.3 |
| Moderate | +50 | 0/1 | 15 | 7,722 | 1,131.0 |
| Moderate | +100 | 1/0 | 34 | 14,859 | 1,481.7 |
| Moderate | +120 | 0/1 | 30 | 7,605 | 1,050.3 |
| Poor | −120 | 1/0 | 45 | 5,265 | 526.2 |
| Poor | −100 | 1/0 | 43 | 5,382 | 537.8 |
| Poor | −50 | 0/1 | 23 | 2,340 | 399.0 |
| Poor | 0 | 0/1 | 28 | 2,457 | 390.6 |
| Poor | +50 | 0/1 | 28 | 2,457 | 390.6 |
| Poor | +100 | 0/1 | 38 | 2,808 | 382.9 |
| Poor | +120 | 0/1 | 46 | 3,042 | 402.7 |

The single-session cells are diagnostic rather than statistical acceptance
thresholds. In particular, the zero-ppm Good control itself dropped while
several signed-offset Good cells completed, and the known zero-ppm session
benchmark already has nonzero Good/Moderate drop rates. The results therefore
do not support attributing individual session drops to SCO. Good/Moderate
goodput at completed signed-offset cells is comparable to the zero-ppm completed
Moderate control. Poor's two negative stress cells completed while its zero-ppm
control and the other offsets dropped, further demonstrating that a one-session
cell cannot support a monotonic SCO-attribution claim.

### Decision

No follow-up SCO-correction design is warranted from this measurement. The
shipped per-frame tracker is adequate throughout the declared ±100 ppm envelope
and at the ±120 ppm stress points: clean AWGN has no observed degradation,
fading-cell differences remain within measurement uncertainty, and no new
synchronization or header-decoding failure pattern appears. Session outcomes are
dominated by the repository's already-documented fading/ARQ baseline. This is
reproducible simulated evidence using linear-interpolation clock mismatch, not a
claim of live two-radio or hardware-clock validation.

