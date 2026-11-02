> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — RateLoop capacity-metric level-bias: root cause isolated to per-level PAPR clip targets

Follow-up to the section directly above, which redirected suspicion for the AWGN-only
`channel_capacity` level-dependence away from fading-coherence time and toward "something in the
per-level measurement path itself," naming pilot density, equalizer path, and clip-induced
noise-variance bias as candidates. This entry runs that investigation down.

**Hypothesis.** `SPEED_LEVELS[level].papr_target_db` is a *deliberately* level-dependent hard-clip
threshold (6.0 dB at BPSK up to 14.0 dB at 64-QAM — see `docs/LIMITATIONS.md`'s PAPR bullet) applied once to
the whole frame's time-domain samples in `CoppaModem::modulate_mapped`'s TX-conditioning chain
(`papr_clip`, after RC-overlap windowing, before the optional HF bandpass). This clip acts on
*every* sample in the frame — preamble, the full-comb probe symbol, the header, and every payload
OFDM symbol's pilots — at one shared per-frame threshold (`rms * 10^(target_db/20)`). A tighter
threshold (closer to RMS) means more samples get clipped, which distorts the probe/pilot
subcarriers that `probe_calibration` and the per-symbol Kalman tracker read their noise variance
from. Since low levels (BPSK) get the harshest clip (6.0 dB) and high levels (64-QAM) get the
loosest (14.0 dB), this predicts an SNR-independent, level-dependent noise floor baked directly
into the pilot channel estimate — worse (higher apparent noise, lower capacity) at low levels,
better at high levels — that only becomes visible once thermal AWGN shrinks enough to stop masking
it (exactly the high-SNR regime where the prior section's trend was strongest).

**Falsifiable prediction.** If this is the mechanism, forcing every level through the *same* clip
target — holding modulation order, pilot pattern, symbol content, and everything else exactly as
production uses them — should collapse the level1→level10 gap in **both** directions: giving low
levels the loose 14.0 dB target only level 10 normally gets should raise their capacity; forcing
level 10 through the harsh 6.0 dB target only levels 1/2 normally get should lower its capacity. If
the gap survives a uniform clip target similarly to how it appears naturally, that falsifies PAPR
clipping as the mechanism.

**Method.** New `crates/coppa-bench/examples/papr_clip_level_bias.rs`: for each of the 9 real
speed levels, builds ONE fixed set of constellation-mapped payload symbols (pseudorandom bits
through that level's real mapper, `CODED_BLOCK_LEN.div_ceil(bits_per_symbol)` symbols — the same
count `CoppaTransceiver::transmit` produces for one codeword) and modulates it three ways, varying
*only* `papr_target_db`: `natural` (this level's real `SPEED_LEVELS` target — reproduces what
production ships), `uniform_loose` (every level forced through 14.0 dB, level 10's real target),
and `uniform_harsh` (every level forced through 6.0 dB, level 1/2's real target). Each signal is
AWGN-faded 40 times per SNR (6/12/18/24/30 dB, seed `0xCA11B` — same grid, trial count, and
significance-check convention as the prior section's bench) and demodulated via the real
`CoppaModem::demodulate_frame`.

**Real SUMMARY output:**

```
SUMMARY (per condition, averaged across SNR grid):
  natural: level1->level10 mean capacity gap = +1.554 bits/s/Hz (2*SE bound = 0.125) -> LIKELY REAL TREND
  uniform_loose: level1->level10 mean capacity gap = -0.117 bits/s/Hz (2*SE bound = 0.134) -> within noise
  uniform_harsh: level1->level10 mean capacity gap = -0.061 bits/s/Hz (2*SE bound = 0.119) -> within noise
```

**Interpretation.** `natural`'s +1.554 bits/s/Hz gap (2×SE bound 0.125) closely reproduces PR #51's
independently-measured +1.581 (bound 0.121) using a completely different symbol-construction path
(pseudorandom mapper output here vs. real LDPC-encoded payloads there) — confirming the effect
isn't an artifact of either bench's specific payload content. Both uniform conditions collapse the
gap to **within noise, in both directions**: giving every level the loose target does not leave a
residual climb (it's slightly negative, −0.117), and giving every level the harsh target does not
leave a residual climb either (−0.061) — if the bias were coming from somewhere else in the
per-level measurement path (e.g. pilot density or an equalizer/noise-estimator effect intrinsic to
modulation order) it should have survived at least partially in these matched-clip conditions,
since only the clip target was pinned equal, not that path. It didn't: at 30 dB, `natural` spans
8.560 (level 1) to 11.451 (level 10) — a 2.9 bit/s/Hz range — while `uniform_harsh` at the same SNR
spans only 8.450–8.625 (0.18 bit/s/Hz) and `uniform_loose` spans 11.392–11.720 (0.33 bit/s/Hz),
both roughly an order of magnitude tighter. **This closes the "which stage" question the prior
section left open: the per-level `papr_target_db` schedule, not pilot extraction, equalization, or
a modulation-order-intrinsic noise-variance effect, is the dominant driver of the AWGN
level-dependence.**

A small residual is worth flagging honestly rather than glossing over: within both uniform
conditions, level 4 (QPSK 3/4) reads consistently highest and level 9 (64-QAM 2/3) consistently
lowest of the 9 levels (e.g. `uniform_harsh`/30 dB: level 4 = 8.625 vs. level 9 = 8.450, a 0.175
spread) — small, well inside each condition's own within-level trial noise at the endpoints used
for the headline gap, but a consistent enough ordering across both uniform conditions that it may
be a second, much smaller effect (plausibly the fixed pseudorandom bit-content differing by level,
producing a slightly different composite time-domain peak profile before the shared clip threshold
applies) — not investigated further here, and irrelevant to the headline finding since it is roughly
an order of magnitude below the `natural` gap it was meant to explain.

**Not fixed — this is the honest, intentional design tension underneath the bias.** The per-level
PAPR clip schedule is not a bug: BPSK's tighter headroom vs. 64-QAM's looser one is a deliberate,
already-tuned real-hardware TX engineering tradeoff (see the PAPR bullet's history), and equalizing
it across levels to remove this bias would cost real EVM/goodput at the levels it's currently tuned
for — not a "trivial, obviously-safe one-line fix" per this task's own bar for when to patch rather
than only diagnose. The actual gap is one layer up: `coppa_ml::channel_capacity` and
`recommend_speed_level` treat a measured capacity reading as if it were a level-independent
"channel truth," when in fact it is measured through a per-level self-noise floor the TX itself
deliberately introduces and that varies by level in a fully known, deterministic way (the
`SPEED_LEVELS` table). A future fix would need to either calibrate `SPEED_LEVEL_MIN_CAPACITY`'s
per-level thresholds to already account for each level's own clip-induced floor (closer to what the
existing calibration may already partially absorb, since it was sound per-level, not
cross-level-compared) or make the closed-loop recommender's *cross-level* comparison
clip-floor-aware before trusting a raw capacity reading taken at one level to predict performance at
another. Left open for the next pass, per this task's diagnose-only scope; `RateLoop`,
`coppa_ml::mcs`, and the `SPEED_LEVELS` PAPR table are unchanged by this investigation.

Reproduce: `cargo run -p coppa-bench --release --example papr_clip_level_bias` (a few seconds
wall-clock — 9 levels × 3 conditions × 5 SNRs × 40 trials of the modem's own OFDM modulate/AWGN/
demodulate path, no fading).

