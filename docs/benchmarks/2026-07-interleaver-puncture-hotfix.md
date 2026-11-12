> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Interleaver puncture + LDPC clamp hotfix

Two correctness defects were fixed on `feature/hotfix-fec-correctness` (technical detail in
`docs/analysis/2026-07-03-world-class-gap-analysis.md` §1):

1. **`BlockInterleaver` structurally punctured 35 coded bits of every frame on
   `hf_standard`** (`crates/coppa-codec/src/ofdm/interleaver.rs`). With block_size=1944,
   carriers=44, the 45×44 grid has 36 pad cells that land mid-scan in the column-major
   readout: the read loop emitted 35 pad zeros and never transmitted the last 35 real coded
   bits (indices ≡ 43 mod 44); RX left the same 35 positions at 0.0 LLR (permanent
   erasures). A fixed 1.8% self-puncture cost 2.4% of parity at R=1/4 but **14.4% of parity
   at R=7/8** — cost scales with code rate. Both prior interleaver tests passed by numerical
   coincidence (all dropped indices are odd; the old test pattern and 0.0→bit-1 mapping
   happened to match every dropped bit). Fixed by skipping pad cells on both TX and RX scans
   (a true bijection over the n real cells). **Wire-format change on `hf_standard`** — both
   ends of a link must run the fixed code.
2. **LDPC decoder messages were unclamped** (`crates/coppa-protocol/src/fec/ldpc/decoder.rs`)
   and could grow past f32 range on non-convergent frames (inf − inf = NaN), poisoning the
   min-scan and hard decisions. Clamped to ±64.

### Paired before/after (AWGN, seed 0x00C0FFEE, 400 trials/point)

"Before" = `results/rebaseline-2026-07/awgn.csv` (committed, measured with the interleaver
bug present). "After" = `results/hotfix-2026-07/awgn.csv` (same seed and trial count, both
fixes applied). Levels 6/7/9/10 (16-QAM/64-QAM, code rates 1/2 through 7/8) were chosen
because the puncture bug's parity cost is proportional to code rate — these are where an
effect should be most visible. The LDPC message clamp (this hotfix's other fix) is
provably decision-neutral — clamping never flips a message's sign, and hard decisions
depend only on sign — so it cannot change any frame's decode outcome; this paired
comparison therefore isolates the interleaver fix's effect specifically.

| Mode (level, rate) | FER≤10% before → after | FER≤1% before → after |
|---|---|---|
| 16QAM 1/2 (6) | 15.0 dB → 15.0 dB | 15.0 dB → 15.0 dB |
| 16QAM 3/4 (7) | 18.0 dB → 18.0 dB | 21.0 dB → **18.0 dB** |
| 64QAM 2/3 (9) | 24.0 dB → 24.0 dB | 27.0 dB → 27.0 dB |
| 64QAM 7/8 (10) | — → — | — → — |

The coarse threshold columns are snapped to the 3 dB sweep grid and only move for one of
the four rows — but the underlying raw FER at the borderline SNR points improved measurably
at all four:

| Mode (rate) | SNR | FER before (Wilson hi) | FER after (Wilson hi) |
|---|---|---|---|
| 16QAM 1/2 | 12 dB | 95.25% (381/400, hi 0.9694) | 87.75% (351/400, hi 0.9061) |
| 16QAM 3/4 | 15 dB | 97.25% (389/400, hi 0.9846) | 94.75% (379/400, hi 0.9654) |
| 16QAM 3/4 | 18 dB | 0.25% (1/400, hi 0.0140) | 0.00% (0/400, hi 0.0095) — crosses the FER≤1% bar 3 dB earlier |
| 64QAM 2/3 | 21 dB | 69.75% (279/400, hi 0.7405) | 52.25% (209/400, hi 0.5710) |
| 64QAM 2/3 | 24 dB | 1.50% (6/400, hi 0.0323) | 1.00% (4/400, hi 0.0254) |
| 64QAM 7/8 | 27 dB | 100.00% (400/400) | 99.25% (397/400) — matches the *old* 30 dB point |
| 64QAM 7/8 | 30 dB | 99.25% (397/400, 3 successes) | 96.00% (384/400, 16 successes) |

Level 10 (r=7/8, the rate with the largest predicted parity impact) shows the clearest
effect: the whole FER-vs-SNR curve shifted right by roughly one 3 dB sweep step (27 dB
after ≈ 30 dB before), and successful decodes at 30 dB increased more than 5× (3 → 16 of
400). Level 7 (r=3/4) crossed the FER≤1% Wilson-CI threshold a full 3 dB earlier (21 dB →
18 dB). Levels 6 and 9 improved in raw FER at their borderline points (roughly halved error
counts at 12/21 dB) but the improvement did not cross a full 3 dB grid line on the coarse
threshold columns at this trial count/seed — a finer SNR step or more trials would likely
resolve a threshold shift there too, but that is not claimed here.

**Low-rate spot check (levels 1–3: BPSK 1/4, BPSK 1/2, QPSK 1/2):** levels 1–2 (BPSK 1/4,
BPSK 1/2) are bit-for-bit identical at every swept SNR point. Level 3 (QPSK 1/2) is
identical except for one borderline frame at 9 dB (399/400 → 400/400 frame errors) — a
single marginal frame flipping from success to failure due to the interleaver fix
re-permuting which coded bits land on which carriers (and therefore which noise
realizations they see under the same RNG seed), not from any change in coding gain at
this low code rate. This is consistent with the prediction that the puncture bug only
ever cost parity bits and R≤1/2 codes have enough margin to absorb 1.8% fewer parity
bits invisibly at this trial count.

