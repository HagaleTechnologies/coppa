> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — RateLoop capacity/selectivity level-bias correction

Follow-up to the two sections directly above (PR #51 confirmed the AWGN level-dependence; PR #52
root-caused it to `SPEED_LEVELS[level].papr_target_db`, both diagnose-only). This entry lands the
fix: `coppa_ml::recommend_speed_level` now takes the frame's originating speed level and corrects
its raw `channel_capacity`/`channel_selectivity` readings for that level's known PAPR-clip-induced
self-noise floor before the calibrated lookup, using a `(level × SNR)` correction table measured by
the new `crates/coppa-bench/examples/capacity_level_correction_calibration.rs` (profile `robust`,
seed `0xCA11B`, 200 trials/cell, AWGN only). The bias turned out not to be a flat per-level offset —
it grows with SNR (e.g. level 10's correction is +0.10 bits/s/Hz at 6 dB but +3.30 at 30 dB) — so
the correction interpolates over the same 5-point SNR grid the diagnostic benches used, on the
frame's own already-computed SNR estimate. `channel_selectivity` is separately level-biased in the
same compounding direction (lower apparent selectivity at higher levels further inflates
`select_speed_level_2d`'s effective capacity) and is corrected the same way. Both correction tables
are reproduced in full in `crates/coppa-ml/src/mcs.rs`'s `CAPACITY_LEVEL_CORRECTION`/
`SELECTIVITY_LEVEL_CORRECTION` consts.

**Scope, unchanged from the design doc**: `SPEED_LEVELS[level].papr_target_db` and all TX
conditioning code are untouched — the per-level clip schedule remains an intentional, already-tuned
tradeoff. The correction is derived entirely from AWGN measurements (the one channel PR #51 confirmed
the bias on) and applied universally regardless of actual channel type, since the receiver has no
ground truth about which channel produced a given frame — a known, explicitly-accepted
approximation for the Watterson case.

**Real re-measured `closed_loop_arq` numbers** (same bench, same acceptance bar as before —
adaptive/best-fixed > 1.0, adaptive/oracle >= 0.8):

```
=== Closed-loop adaptive rate (300 frames, robust profile) ===
adaptive throughput : 286288 bits
best fixed (L4)     : 357424 bits
per-frame oracle    : 428936 bits
adaptive/oracle = 0.667   adaptive/best-fixed = 0.801
```

These numbers are *worse* than the pre-fix baseline (adaptive/best-fixed 0.894→0.801,
adaptive/oracle 0.751→0.667), not better, and the `>1.0`/`≥0.8` bar remains unmet — reported
honestly, per the prior agreement that clearing it is not a condition of landing this PR. The
bench's own level-vs-channel trace narrows down where the regression concentrates: the first two
schedule thirds (the pure-AWGN SNR ramp 3→30→3 dB) track the channel the same way as before, but the
final third (Watterson Good then Poor, both pinned at a nominal 24 dB) is where the loop now drops to
L2 and then L1 and stays there despite the nominal SNR holding flat at 24 dB the whole time — visibly
worse than "slow to climb back," an active, sustained drop. This is consistent with the design doc's
explicitly flagged, known approximation: the correction table is derived exclusively from AWGN
measurements and applied unconditionally to every frame regardless of actual channel type, since the
receiver has no ground truth about which channel produced a given frame. Watterson fading's real
per-carrier noise-variance profile (deep frequency-selective fades, not AWGN's flat floor) is not the
regime the correction was calibrated against, and applying an AWGN-derived, SNR-keyed correction to a
Watterson-faded reading during this bench's fading tail appears to over-subtract capacity at exactly
the levels/SNRs where the loop had climbed high beforehand — actively hurting performance in the one
regime the design doc called out up front as "a reasonable extrapolation, not a validated one." This
is a real, measured regression against the specific pre-fix numbers CLAUDE.md and the "Phase 3 Task 4"
section above recorded, concentrated in the Watterson-fading portion of this particular bench
schedule, not a general worsening of the AWGN-only case the correction was built and validated
against.

**Correction on the "no ground truth" framing above, and a concrete next step.** A final
whole-branch review pointed out that "the receiver has no ground truth about which channel produced
a given frame" overstates the case: `channel_selectivity(noise_vars)` is already computed on the
very next line inside `recommend_speed_level` and IS a cheap, already-available discriminator
(near-zero on AWGN's flat floor, substantially higher under frequency-selective Watterson fading) —
it just isn't used to gate or scale the correction today. The most promising untried lever for
closing this Watterson-tail regression is a selectivity-scaled correction: attenuate the AWGN-derived
correction as raw selectivity rises above the AWGN floor, rather than gating it to zero on fading,
since the underlying PAPR-clip self-noise bias is a TX-side effect present on every channel type,
not an AWGN-specific artifact — a hard on/off gate would under-correct the real bias on fading, not
just avoid over-correcting it. Not attempted here; left as the next concrete step for whoever picks
this up.

No regression observed on `tests/phase_c_loopback.rs`'s full-ladder AWGN/Watterson FER sweep (Step
2 above) — the correction, applied only inside `recommend_speed_level`, does not affect
`channel_capacity`/`channel_selectivity`'s other callers or the LDPC-facing noise-variance path.
`test_snr_fer_monte_carlo` was re-run in full and still shows FER=0.00/100 for every level (1, 3-7,
9-10) across its whole swept SNR range, unchanged from CLAUDE.md's documented baseline.
(COP-70 correction: that sweep could not fail, since its full-band noise was ~9 dB optimistic
against the 3 kHz convention, its window sat above every level's waterfall, and it asserted
nothing, so this sentence is not evidence of the absence of a regression.)

Reproduce: `cargo run -p coppa-bench --release --example capacity_level_correction_calibration`
(calibration table) and `cargo run -p coppa-bench --release --example closed_loop_arq` (impact
measurement).

