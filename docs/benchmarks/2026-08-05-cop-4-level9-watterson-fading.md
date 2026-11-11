> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## Level 9 Watterson fading diagnosis (COP-4)

Measured at revision `be2141b` with deterministic seed `0x00c00004`:

```bash
TRIALS=300 cargo run --release -p coppa-bench --example level9_profile_ab
TRIALS=500 cargo run --release -p coppa-bench --example level9_ground_truth_admissibility
TRIALS=60 cargo run --release -p coppa-bench --example per_frame_link_diagnosis -- default 9
```

The profile matrix covered AWGN plus Watterson Good, Moderate, and Poor at
6–36 dB in 3 dB steps. Its five explicit profile arms were `vhf_wide`,
`vhf_wide_long_cp` (CP-only change), `vhf_narrow`, `hf_standard`, and
`hf_robust`; every cell used 300 trials.

### Failure mode and corrected per-frame instrument

The ladder now preserves `ReceiveError` instead of discarding it. At 30 dB on
the production `vhf_wide` profile, level 9 failed 255/300 Good frames, of which
254 were `LdpcNotConverged`; Moderate was 280/300 (274 LDPC, 6 sync), and Poor
was 288/300 (283 LDPC, 5 sync). The floor is therefore payload/FEC dominated,
not acquisition dominated, and remains approximately SNR-independent.

The former levels-6/9 conclusion at the older per-frame section is retracted:
its BPSK sign decision and legacy LDPC reconstruction produced about 49.5% raw
BER for higher-order modulation even on clean AWGN. With production
`demap_hard` and the NR-BG2 chain, a 60-trial level-9 run at 30 dB reports
Good raw BER 20.52% (high-NV 23.37%, low-NV 17.71%), Moderate 20.05%
(24.80%/14.33%), and Poor 29.39% (29.77%/27.58%). Errors are generally more
concentrated where the receiver assigns high noise variance: the equalizer is
identifying fades, but the present code/interleaver does not cover them.

### Profile × CP result

No fading arm cleared the Wilson-95%-bounded FER≤10% target at any SNR through
36 dB. On Good, production `vhf_wide` peaked at 991 bps; changing CP alone to
300 samples reduced peak goodput to 910 bps, `vhf_narrow` reached 780 bps,
`hf_standard` 439 bps, and `hf_robust` 401 bps. On Poor the respective peaks
were 264, 161, 291, 417, and 411 bps. A longer CP and denser pilots therefore
do not rescue level 9; `vhf_wide` was an unrecorded variable in the historical
measurement, not the demonstrated cause. The result is consistent with the
existing forced-`hf_standard` golden-vector and MIL-STD measurements.

### Perfect-CSI admissibility bound

The decode-independent oracle uses the Watterson model's exact tap gains,
perfect synchronization/CSI, the production 64-QAM mapper, NR-BG2 rate match,
and block interleaver. It uses the harness's clean-referenced 3 kHz noise-band
convention. At 30 dB on Watterson Good and `vhf_wide`, 458/500 frames were
admissible: 91.6%, Wilson 95% CI 88.84–93.73%. The real receiver at the same
profile/preset/SNR decoded 45/300: 15.0%, Wilson 95% CI 11.40–19.48%.
Moderate's bound was 91.4% (88.62–93.55%) versus 6.67% real success. Poor's
flat-per-carrier result is diagnostic only because its 96-sample delayed tap
exceeds the 60-sample CP and the oracle does not model residual ISI.

Assumptions: the CP absorbs multipath (true for Good/Moderate, false for Poor),
the channel is static within an OFDM symbol, CSI and sync are perfect, and the
oracle is an optimistic isolated-FEC bound. The current oracle analytically
matches the clean-referenced noise convention; it does not include transmitter
PAPR-clipping self-noise, so the large bound-versus-receiver gap is evidence of
headroom, not a predicted production FER curve.

### Decision

Decision rule **R2** fired: the perfect-CSI bound and real decode confidence
intervals are disjoint by a wide margin. The level-9 fading gap is not a proven
physical 64-QAM ceiling, nor is it fixed by switching profiles or extending CP.
The evidence points to fade-diversity/FEC coverage as the next narrow design
lever. No production routing or waveform change is made by COP-4's diagnostic
commits; such a change needs an interop-aware design and identical before/after
measurement rather than speculative tuning inside this investigation.

