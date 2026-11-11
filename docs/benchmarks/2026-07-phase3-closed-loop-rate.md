> Moved from BENCHMARKS.md by COP-146. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## 2026-07 — Phase 3 Task 4: closed-loop adaptive rate

**What shipped:** the receiver now recommends a per-frame speed level
(`coppa_ml::recommend_speed_level`, wrapping the existing
`channel_capacity`/`channel_selectivity`/`select_speed_level_2d` selectors over
the frame's own per-carrier noise variances) as the third element of
`CoppaTransceiver::receive()`'s return; that level is fed back to the sender on
the ACK (`TransportPdu::new_ack_with_rate`/`suggested_rate()`); the sender's
`coppa_ml::RateLoop` applies it with hysteresis (raise one level only after
`raise_dwell` consecutive higher recommendations; drop immediately to the
recommendation on a lower one, or one step on any delivery failure/ARQ
timeout). Validated by `crates/coppa-bench/diagnostics/closed_loop_arq.rs` — a
simulation-only bench (no daemon/audio wiring; that remains deferred, per the
plan). The old, dead `coppa-engine::RateController` (never wired to anything
beyond a debug log) and the aspirational, unused, SNR-only `coppa-ml::MCS_TABLE`
are deleted as part of this work.

**Bench schedule:** 300 simulated frames, `hf_robust` profile — AWGN SNR ramps
3→30→3 dB over the first two-thirds, then Watterson Good (24 dB) then Watterson
Poor (24 dB) fading over the last third.

**Measured result — the plan's acceptance bar is NOT cleanly met:**

| raise_dwell | adaptive/best-fixed | adaptive/oracle |
|---|---|---|
| 3 | 0.887 | 0.744 |
| 4 | 0.891 | 0.747 |
| **5 (shipped default)** | **0.894** | **0.751** |
| 6 | 0.893 | 0.750 |
| 8 | 0.873 | 0.732 |
| 10 | 0.849 | 0.713 |
| 12 | 0.840 | 0.705 |
| 15 | 0.836 | 0.702 |

`raise_dwell = 5` is a genuine peak (both ratios rise from 3→5 and fall on both
sides), not just "more damping is safer" — so this isn't a case of needing
further hysteresis tuning; the acceptance target (adaptive/best-fixed > 1.0,
adaptive/oracle ≥ 0.8) is out of reach via `RateLoop`'s parameters alone at this
operating point.

**Root cause, from an ad-hoc (not committed) diagnostic — a well-reasoned
hypothesis, not independently-verifiable fact until a reproducible bench
exists:** the per-carrier capacity metric this recommendation is built on
(`channel_capacity`/`noise_vars`, already used elsewhere and calibrated before
this task) appears not invariant to which speed level the measured frame
happened to use — at a fixed, true, injected AWGN SNR (no fading), averaged
over 30 seeds via temporary, uncommitted probes, a level-7 transmission's own
channel estimate read several dB higher "capacity" than an identical channel
measured via a level-1 or level-2 transmission (both `hf_standard` and
`hf_robust`, independent of turbo re-estimation). This measurement was not
committed as a reproducible script/test — a follow-up should either commit it
or treat this explanation as a hypothesis consistent with the evidence below,
not settled fact. But `SPEED_LEVEL_MIN_CAPACITY`
(the table `select_speed_level_2d` looks up into) was calibrated exclusively via
a *fixed* level-2 probe frame (`mcs_calibration.rs`/`adaptive_mcs_validation.rs`'s
`sound_capacity`, which always transmits the sounding frame at level 2
regardless of what's being evaluated) — those existing, already-validated
benches never expose the level-dependence because they never vary the probing
level. This task's design (recommend from the actual in-flight frame, at
whatever level it used — the whole point being zero extra probe overhead)
does vary it, and that's exactly what exposes a self-reinforcing bias: the
higher `RateLoop` climbs, the more inflated its own next capacity reading
becomes, so it keeps being told to climb regardless of the real channel. This
is consistent with (and plausibly a further symptom of) the channel-estimation
issue already tracked as open in this document's "Known Limitations" (see
docs/LIMITATIONS.md); it is **not** a bug in `RateLoop`'s hysteresis logic itself, and
fixing the underlying metric is out of this task's scope (it belongs to the
channel-estimation / MCS-calibration layer, not the rate-loop controller). See
`crates/coppa-bench/diagnostics/closed_loop_arq.rs`'s module doc for a summary of the
capacity-bias hypothesis and its caveats. Two unrelated real bugs were found
and fixed in the bench itself along the way (a
constant `seq_num` corrupting IR-HARQ's per-seq accumulator across
logically-independent simulated frames, and a shared transceiver risking the
same cross-contamination across fixed-level runs).

**What still works, qualitatively:** the level trace does track the schedule's
shape (climbing through the AWGN ramp-up, falling on ramp-down, dropping
sharply into the Watterson-Poor tail) — the closed loop is not broken or
inert, it is measurably miscalibrated at this specific combination of profile
and schedule.

