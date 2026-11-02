> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## COP-6 LDPC decode CPU

Measured at `2dc3b99` on the aarch64 Apple M-series development host with
`cargo run -p coppa-bench --release --example ldpc_decode_timing` (release
profile) and the safe-Rust, edge-outer/sub-row-inner BG2 decoder. The decoder now stores only a 1.9 KiB
row/edge graph description instead of the former 271 KiB variable-index table.
The output is bit-identical to the frozen legacy loop over the complete 9,152
element posterior in the committed characterization tests.

The permanent `ldpc_decode_timing` harness's realistic-payload column produced
(the realistic input pins the known unused payload bits, matching production):

| Level | old rate | timing SNR | old µs/frame | new µs/frame | ratio | avg iterations | ≤3x |
|---:|:---:|---:|---:|---:|---:|---:|:---:|
| 1 | 1/4 | 3.0 | 137.08 | 202.25 | 1.48x | 2.00 | met |
| 2 | 1/2 | 5.0 | 58.98 | 203.35 | 3.45x | 2.00 | NOT met |
| 3 | 1/2 | 8.0 | 116.40 | 197.11 | 1.69x | 2.00 | met |
| 4 | 3/4 | 10.5 | 48.19 | 110.58 | 2.29x | 1.00 | met |
| 5 | 2/3 | 13.5 | 51.88 | 224.46 | 4.33x | 2.00 | NOT met |
| 6 | 1/2 | 14.5 | 68.24 | 228.09 | 3.34x | 2.00 | NOT met |
| 7 | 3/4 | 18.0 | 55.05 | 139.64 | 2.54x | 1.00 | met |
| 9 | 2/3 | 20.5 | 66.58 | 243.81 | 3.66x | 2.00 | NOT met |
| 10 | 7/8 | 25.0 | 56.88 | 159.37 | 2.80x | 1.00 | met |

The same run's legacy-input ratios were 1.52/3.36/3.38/2.37/2.19/3.37/
2.37/4.20/2.76x for levels 1/2/3/4/5/6/7/9/10 (six of nine met). That
column is retained as a method-comparison control, not the verdict source:
`payload_bits=0` omits the production decoder's known-pad pinning and changes
both convergence behavior and the old-codec denominator.

Levels 5 and 9 (both old-codec rate 2/3) are the worst cells at roughly
4.1-4.3x across repeated idle-host runs. Levels 1, 3, 4, 7,
and 10 meet the accepted ≤3x budget; levels 2, 5, 6, and 9 do not. The same
harness's forced-iteration linear fit had a negative intercept. That physically
impossible fixed cost means this small-sample linear model is misspecified; its
slope cannot support a single-number marginal improvement claim. The earlier
302.8 µs/iteration baseline was itself reconstructed from a profiler rather
than measured by this harness, so no code-only percentage is claimed. Likewise,
the previously reported 12.3% wrapper overhead
compared different punctured inputs (`±3.0` versus `0.0`) and was not an
apples-to-apples wrapper measurement.

These ratios supersede the older `task4_bg2_ldpc_gate` timing table. The verdict
change from its published 3.36-10.06x range is predominantly a measurement
correction: the canonical harness warms both decoders and uses pinned realistic
payloads. COP-6's code changes are not credited with that multi-fold shift. The
older gate now warms both decoders too and remains only as a historical
FER/OFDM diagnostic; `ldpc_decode_timing` is the CPU acceptance instrument.

One planned scalar experiment stored all pass-A extrinsics and hoisted the
scale multiplication. It regressed the fit to 301.31 µs/iteration and was
reverted. The remaining gap is therefore a safe-portable implementation and
fixed-graph-size ceiling for this attempt. Reusable per-decoder scratch buffers
remain an untried safe/portable follow-up before SIMD or unsafe work;
wider explicit SIMD, graph changes, or build/unsafe policy changes remain
separate decisions.

