> Moved from BENCHMARKS.md by COP-88. Unchanged; it uses the conventions of its date. Current numbers and conventions: [BENCHMARKS.md](../../BENCHMARKS.md).

## Short-CP under closed-loop rate adaptation (COP-2)

Does the short-CP airtime saving move `RateLoop`'s stuck closed-loop metric? COP-2 asked this so the
project could route its next effort either to the short-CP lever or to fade-diversity interleaving —
the two levers docs/LIMITATIONS.md names as untried. The answer has three parts, and the first is settled by
arithmetic rather than by measurement.

### Conventions changed — prior `closed_loop_arq` figures are NON-COMPARABLE, not superseded

Following this file's own precedent for a measurement-convention change, the old numbers stay in
place and are marked non-comparable rather than re-based. **Two independent reasons**, both of which
must travel with those figures wherever they are requoted:

1. **Metric.** Every prior `closed_loop_arq` figure (0.894/0.751, 0.801/0.667, 0.931/0.775,
   0.914/0.762, and the 326680 / 357424 / 428936 bit totals) counts delivered info bits per frame
   **slot**. The primary metric is now airtime-normalized goodput in **bits per second** — this
   file's canonical `payload_bits × (1 − FER) / frame_airtime`. The bits-per-slot convention was the
   single exception to that canon in the whole repo, and the example said so in its own module doc.
2. **Profile base.** The CP-contrast arms run on the `hf_standard` family, because
   `hf_standard` ↔ `hf_standard_short_cp` is the only exact CP-only profile pair in the workspace.
   Every prior figure was measured on `hf_robust` (36 data / 12 pilot), which differs in more than CP
   length and has no short-CP twin anywhere.

The metric change **alone** moves `best_fixed` from L4 to L7 on `hf_robust` — from 1334.9 to 1946.0
bps, a +45.8% denominator — with **zero CP involvement**. That is why no CP delta below is ever
reported against 0.914/0.762.

**The legacy control is exact.** Re-running the pre-COP-2 quantities under a controlled condition
(the one deliberate waveform-affecting change reverted — see the note below) reproduces the record to
the bit: adaptive **326680** / best fixed (L4) **357424** / oracle **428936** / `a/o` **0.762** /
`a/bf` **0.914**. The metric swap is therefore provably accounting-only and perturbed no link
dynamics.

In the normal (unreverted) run the same figures read 329328 / 358848 / 429096 / 0.767 / 0.918 — a
0.04–0.81% drift. That residual was attributed **by controlled experiment rather than by argument**:
`make_header` now reads `tx.profile().bandwidth_id` instead of the old hardcoded `1`, mirroring
`CoppaCore::encode_bytes`, and `hf_robust`'s real `bandwidth_id` is **3**. The corrected header
changes the header codeword and so flips a couple of borderline decodes. Hardcoding `1` back
reproduces the record exactly; the fix is kept because a CP-adaptive arm transmits under two
`bandwidth_id`s in one run and the old constant was simply wrong.

### Table 1 — the ratio-invariance result (analytical, not measured)

`frame_airtime_s = total_syms(level, profile) × (fft_size + cp_samples) / sample_rate`, and
`total_syms` depends on the profile **only** through `data_carriers_per_symbol` — identical for
`hf_standard` and `hf_standard_short_cp` (both 44 data / 4 pilot). So a uniform CP change is one
constant divisor, independent of the level sequence:

| bits/symbol | levels | total_syms | long-CP airtime (s) | short-CP airtime (s) | short/long |
|---|---|---|---|---|---|
| 1 | 1, 2 | 52 | 1.36500 | 1.19600 | 0.876190 |
| 2 | 3, 4 | 30 | 0.78750 | 0.69000 | 0.876190 |
| 3 | 5 | 22 | 0.57750 | 0.50600 | 0.876190 |
| 4 | 6, 7 | 19 | 0.49875 | 0.43700 | 0.876190 |
| 6 | 9, 10 | 15 | 0.39375 | 0.34500 | 0.876190 |

Exactly `1104/1260 = 0.876190476…` at **every** level (inverse `1260/1104 = 1.141304…`: 12.381% less
airtime, +14.130% more goodput). Pinned by `coppa_protocol::modem::airtime`'s
`short_cp_scales_frame_airtime_by_one_constant_at_every_level`, with
`short_cp_and_long_cp_share_the_same_symbol_count_per_level` guarding the factorization the first
test rests on.

**Consequence, arithmetic rather than prediction:** a constant cancels out of a ratio, so
`adaptive/best-fixed` and `adaptive/oracle` — ADR-008's bar — **cannot move on the airtime saving at
all**, once the comparator set is allowed the same lever. Any ratio movement under short CP is a FER
or `RateLoop`-trajectory effect. This is the same observation the short-CP coherence-time section
above already made from the other direction: its AWGN +14.1% / +14.2% peak-goodput figures were "the
harness behaving as expected, not a coherence-time result."

### Table 2 — the bar on both conventions, and the false pass

5 seeds × 300 frames, 22 runs per seed per base. `standard` is the production-faithful pair (D1);
`robust` pairs `hf_robust` against a bench-local **synthetic** short-CP twin with an unallocated
`bandwidth_id` (D1c) — it answers the ticket's literal question with zero base-profile confound but
measures the lever's *magnitude*, not a shippable feature.

| Quantity | Required | Old (bits/slot) | `standard` joint | `robust` joint |
|---|---|---|---|---|
| `adaptive/best-fixed` | `> 1.0` | 0.914 | **0.948** (1/5 seeds) | **0.842** (0/5 seeds) |
| `adaptive/oracle` | `>= 0.8` | 0.762 | **0.654** | **0.586** |
| best-fixed level/cell | — | L4 (by bits) | shortL7 (shortL9 seed 0) | shortL7 |

**The bar is NOT MET on either base.**

**And here is why D4 was pre-committed.** Scored against the 9-cell long-CP-only comparator — the one
*denied* short CP — the **same arm B** on the `standard` base clears `> 1.0` on **all five seeds**:

| seed | `B/bf(long)` (denied lever) | `B/bf(joint)` (pre-committed) |
|---|---|---|
| 0 | 1.066 | 0.924 |
| 1 | 1.060 | 0.905 |
| 2 | 1.047 | 0.926 |
| 3 | 1.131 | 0.970 |
| 4 | 1.203 | 1.014 |
| **mean** | **1.101 — 5/5 clear** | **0.948 — 1/5 clear** |

The gaps to the bar are +9.41% (0.914 → 1.0) and +4.99% (0.762 → 0.8); the uniform CP factor alone is
+14.130%, roughly 1.5× and 2.8× the required move. So that "pass" is pure denominator arithmetic.
**This is the pre-registered reward-hack observed live, not hypothesized** — and it is the single most
important number in this section.

> **Cost-model caveat on the "5/5".** Every figure in this table is arm B at switch-cost ×0 (data
> airtime only). Charging the handshake at ×1 costs arm B ~3.3–4.5%, which puts seed 2 (1.047) at
> roughly 1.00 — so a strict `>` could read 4/5 rather than 5/5. Treat the *shape* as the finding, not
> the count: the gaps above are 1.5–2.8× the CP constant either way. See Table 3's switch-cost
> paragraph.

### Table 3 — arms, comparators, and the denominator delta (D4a's primary answer)

| Quantity | `standard` | `robust` |
|---|---|---|
| arm A (long-CP control) | 1838.3 bps | 1506.2 bps |
| arm P (rebuild placebo) | 1838.3 bps (**+0.00%**) | 1506.2 bps (**+0.00%**) |
| arm B (CP-adaptive) — *data airtime only, = ×0* | 2399.8 bps | 1912.4 bps |
| arm C (fixed short CP) | 2531.6 bps | 2270.1 bps |
| best-fixed(long) → best-fixed(joint) | 2179.1 → 2531.6 (**+16.18%**) | 1948.3 → 2270.1 (**+16.52%**) |
| oracle(long) → oracle(joint) | 3066.6 → 3670.5 (**+19.69%**) | 2736.5 → 3262.6 (**+19.23%**) |
| arm B vs arm C | −5.21% (B>C on 1/5) | −15.76% (B>C on 0/5) |
| arm B vs arm A | +30.5% (B>A on **5/5**) | +27.0% (B>A on **5/5**) |

**The denominator delta is the ticket's primary answer, and it is real:** +16–17% on best-fixed,
+19–20% on the oracle, consistent across both bases. Both exceed the pure +14.130% CP constant
because the joint comparator also re-picks the *level*, not only the CP mode — "best fixed" means the
best static *configuration*, and short CP shifts which level that is. `best-fixed(joint)` selected a
short-CP cell on 5/5 seeds on both bases.

**Switch cost, and where it is (and is not) in the numbers above.** One switch per run, five on-air
frames. Goodput at ×0 / ×1 / ×6 multipliers (no-charge bound / loss-free nominal / ARQ-budget ceiling
at `DEFAULT_MAX_RETRANSMIT = 5`) on `standard` seed 0: 2301.4 / 2207.8 / 1834.6 bps. Four things are
**not** charged at any multiplier and all four bias arm B optimistic: leg retransmissions, the four
half-duplex turnarounds at `DEFAULT_TURNAROUND = 150 ms`, handshakes that fail and revert via COP-1's
G1-G4 having spent the full budget for zero CP change, and the levels ≥ 5 divergence from production.

Two corrections to how this cost was reported when these tables were first written, both found by the
branch's own PR review and both fixed in the bench (`cp_arm::switch_airtime_s`,
`closed_loop_arq`'s `SeedAgg`); neither moves a verdict above, and **no re-run was needed for
either**, since both are accounting, not measurement:

- **The charge quoted here was the `standard` price applied to both bases.** `switch_airtime_s`
  reconstructed the profile pair from `cp_arm::profile_for`, which is hardcoded to
  `hf_standard`/`hf_standard_short_cp` — so the `robust` runs, whose pair is `hf_robust` (36 data / 12
  pilot carriers), were charged the 44-carrier price. Fewer data carriers means more symbols per frame
  (61 vs 52 at bits/symbol 1), so the true `robust` charge is **~7.61 s (~4.2% of arm B's airtime)**,
  not the ~6.49 s / 3.4–4.7% this paragraph used to quote for both. `standard`'s 6.487 s was correct.
  The function now takes the pair from its caller.
- **Every headline arm-B figure above is the ×0 (data-airtime-only) number.** The switch airtime the
  run computed was accumulated but spent only in the bench's own SWITCH ACCOUNTING block — never in
  the bar ratios, the aggregate mean, or the per-seed deltas, and with no label saying so. Charging it
  at ×1 moves arm B by roughly −3.3 to −4.5% (`standard` seed 0: 2301.4 → 2207.8 bps), which
  **deepens every already-negative arm-B result** rather than changing its sign: `B/bf(joint)` 0.948 →
  ~0.910, `arm B vs arm C` −5.21% → more negative. The bench now prints every one of those quantities
  at both ×0 and ×1 so the band is visible at the point of use; it deliberately does *not* silently
  adopt ×1 as the primary, because this bench cannot measure which multiplier is right.

The one number above that is **not** robust to the cost model is Table 2's "5/5 seeds clear
`B/bf(long) > 1.0`": seed 2 sits at 1.047 under ×0 and lands at ~1.00 under ×1, so a strict `>` could
make it 4/5. That does not weaken the finding it illustrates — the false pass is a *denominator*
artifact, and the +9.41% / +4.99% gaps against a +14.130% CP constant are what carry it — but the
"5/5" itself should not be quoted as though the cost model were settled.

**Arm P is the load-bearing control here.** It rebuilds the transceiver on exactly arm B's switch
frames with the *same* profile, and matched arm A to the printed decimal on all 5 seeds on both
bases. A CP switch in this bench is a transceiver rebuild (there is no profile setter), and this repo
has twice shipped bugs that were invisible to inspection and showed up only as state carried across
that boundary (PR #61/#62's stale IR-HARQ accumulator, PR #69's `calibrated_bias` saturation). The
rebuild's inertness is now measured rather than argued, so arm B is read against arm A.

*Post-hoc hardening (no number moves).* When these runs were taken, arm B's rebuild was gated on
profile *inequality* while arm P's fired on every recorded switch — so on a `from == to` switch (five
control frames for zero CP change, reachable at latency 5) arm P would have rebuilt and arm B would
not, and the placebo would have quietly stopped being a control. It never happened here: this
schedule produces exactly one non-degenerate transition per run (see Table 4), so both arms rebuilt at
frame 9 and the sets matched. It was worth closing anyway, because the divergence was *invisible* —
only arm P's rebuild count was printed — and Table 4's own routed follow-up (re-deriving the 2.5 ms
threshold) is precisely the change that would make drop-backs, and therefore `from == to`, live. Arm
B's rebuild set is now equal to `switches[].effective_from` by construction
(`cp_arm::CpPolicy::applied_on`), both counts are printed, and a mismatch prints a warning.

**Airtime share (D1a).** ~94% of arm B's airtime ran under short CP, split roughly 33–53% at levels
≤ 4 and 42–60% at levels ≥ 5. **Only the levels ≤ 4 share is production-reachable**:
`select_ofdm_profile` routes every level ≥ 5 to `vhf_wide()` and ignores `cp_mode` there, so the
levels ≥ 5 contribution is an explicit **upper-bound overshoot** this bench collects and production
cannot.

### Table 4 — the `CpGate` delay-spread histogram, and why adaptivity was never exercised

`DelayDomainEstimator::delay_spread_ms` is quantized to `(fft_size / nc) / sample_rate` = 20 samples =
**0.41667 ms**, and `fit` clamps the tap count to `1..=8`, so the metric can take only **eight**
values, 0.000–2.9167 ms. The histogram is exhaustive and **`off_grid = 0` in all 20 rows**, which is
what makes it valid rather than approximate.

| Schedule segment | Buckets observed | Undecoded (of 50–100) |
|---|---|---|
| AWGN ramp up | 0.000 only | 0 |
| AWGN ramp down | 0.000 only | 1–2 |
| Watterson Good | 0.000, 0.417 | 13–15 |
| Watterson Poor | 0.000, **2.083** | 10–14 |

**Watterson-Poor measures 2.083 ms — below `CpGate::default_coppa()`'s 2.5 ms threshold, which
requires *strictly* below.** So the gate reads the harshest preset in the schedule as "calm" and can
never drop on it. Measured consequence: **exactly one transition per run**, on every seed and both
bases, always `LongCp → ShortCp` decided at frame 3 and effective at frame 9 — latched at frame 4 and
never dropped across the remaining 291 frames.

Per the **pre-committed transition-count gate**: with ≤ 1 transition this schedule does **not**
exercise CP *adaptivity*. Arm B is therefore fixed short CP with a nine-frame long-CP prologue, and
must not be described as adaptive control. That also explains arm B ≈ arm C, and it means the
`arm B vs arm C` row above measures adaptive *rate* against best fixed *rate* on a common short CP —
not adaptive CP against fixed CP.

For cross-checking: `cp_gate.rs`'s own "2.55–3.2 ms" array is **deliberately synthetic** and says so
in its doc comment — it is not a measurement, and the 2.083 ms figure here is the first real per-frame
delay-spread measurement fed to that gate on this schedule.

### Table 5 — the flip gate (HUMAN DECISION GATE)

The ticket's literal ask was to enable short-CP negotiation by default. It reads as one flag and is
**three**, and the gate is evaluated rather than assumed:

| # | Criterion | Verdict | Evidence |
|---|---|---|---|
| F1 | The `set_speed_level` / `cp_negotiator` desync pre-condition is closed | **Yes** | The `level >= 5` `cp_mode` reset is deleted; the invariant is asserted across a real `RateLoop` VHF excursion in *both* roles through the real `decode_and_dispatch_audio` path; a non-self-healing tracing canary tripwires any future divergence. Pre-fix the daemon test failed with "preamble synchronization failed" — the link was dead. |
| F2 | The lever is real (ratios move favourably, outside the run-to-run band, sign-consistent across all 5 seeds) | **Partly — and not as a ratio** | The *denominator delta* is real and sign-consistent (+16–17% / +19–20%, both bases), and arm B delivered more bits than arm A on 5/5 seeds (+3.73% mean) while using 20.44% less airtime. But the *ratios* do not clear the bar, and per Table 1 they cannot move on airtime at all. CP **adaptivity** specifically added nothing (arm B < arm C on 4/5 and 5/5). |
| F3 | The flip changes observable behaviour | **No** | The propose site is nested inside `if self.config.engine.cp_gate_enabled`; the inbound `CpControl` arm is nested inside `arq_enabled`; all three flags default `false`. Proved by `cp_negotiation_enabled_alone_never_initiates_without_cp_gate`. |

**F3's sharper form, which argues for more caution rather than less:** on a *default* daemon the flag
alone changes nothing observable. But on any daemon that has turned ARQ on — an ordinary, wanted
configuration — flipping `cp_negotiation_enabled` alone makes the station a full **responder**: it
will send `Confirm`, call `engine.set_cp_profile`, send `CpSwitched`, and arm G1-G4, with
`cp_gate_enabled` still off, because `handle_cp_control` gates only on `cp_negotiation_enabled`. So
the correct claim is "never **initiates** without `cp_gate_enabled`", **not** "inert in general" — and
a responder that switches CP is exposed to the desync hazard exactly as an initiator is, which is a
second, independent reason F1 gates F3.

**No boolean default changes in this ticket.** Two alternatives were considered and rejected:

- **Flip `cp_negotiation_enabled` alone.** Zero behaviour change on a default daemon, and actively
  misleading — an operator reading the config would believe negotiation is on when nothing can ever
  propose.
- **Flip it together with `arq_enabled`.** `arq_enabled` gates the daemon's entire reliable-delivery
  data path. That is an unrelated production change no ticket has asked for and that this measurement
  cannot inform.

The ask is **answered, not dropped**: enabling CP negotiation is a three-flag conjunction, the flag
alone is never a proposer, and the default stays `false` pending this gate.
`cp_negotiation_enabled`'s field doc now carries that conjunction as its headline rather than a
parenthetical, and `test_cp_negotiation_requires_two_more_flags_by_default` is the tripwire that makes
a future *partial* flip impossible to land silently.

Reproduce: `cargo run -p coppa-bench --release --example closed_loop_arq` (robust base) and
`... --example closed_loop_arq -- standard` (production pair) — ~75 min wall clock each for the full
5-seed / 22-run sweep on a 10-core Apple Silicon machine, run concurrently.
`COPPA_CL_FRAMES` / `COPPA_CL_SEEDS` shrink it to seconds for a wiring check. **CI compiles bench
examples but never runs them, so these figures carry no regression protection** — the arithmetic they
rest on is unit-tested in `coppa_bench::adaptive_goodput` (14 tests) and `coppa_bench::cp_arm`
(17 tests) precisely because the example itself is not.

### Decision

**ADR-008's bar cannot express an airtime improvement, and that — not a lever verdict — is this
ticket's primary result.** The bar is a ratio; a uniform CP change is one constant divisor; the lever
cancels by construction. The proof is measured, not argued: the same arm B clears `> 1.0` on **5/5
seeds** against a comparator denied short CP and on **1/5** against the pre-committed joint one. Any
future airtime lever will read as "bar NOT MET" on this instrument no matter how much airtime it
saves. This is a COP-5-shaped conclusion — redefine and record, do not force the metric.

**The routed lever is short CP as a static, negotiated configuration for HF levels 1-4 — not as an
adaptive control, and not fade-diversity interleaving.** The evidence: `best-fixed(joint)` picks a
short-CP cell on 5/5 seeds on both bases; the denominator delta is +16–17% (best-fixed) and +19–20%
(oracle); and arm B delivered more bits than arm A on 5/5 seeds while using 20.44% less airtime, so
the FER channel contributes and is sign-consistent. Fade-diversity interleaving stays available but
is not the next lever: short CP demonstrably delivers and is far cheaper. What the evidence does
**not** support is CP *adaptivity* — arm B lost to fixed short CP on 4/5 and 5/5 seeds, and per
Table 4 the gate never dropped, so adaptivity was never actually exercised here.

**A separate, real defect surfaced and is filed rather than folded in:** `CpGate`'s 2.5 ms threshold
is mis-set relative to what `DelayDomainEstimator` actually produces (Watterson-Poor = 2.083 ms), so
the gate cannot drop on the harshest channel this schedule contains. And it gates on measured **delay
spread** while short CP's measured gain mechanism is shorter frame duration under fast **Doppler** —
which the short-CP coherence-time section above already flags as different channel properties that
"need not correlate." So this null adaptivity result may mean *this gate's input is the wrong signal*
rather than *short CP is not a lever*. Re-deriving that threshold against real measured spreads is
its own ticket, and it should precede any attempt to use `CpGate` as a throughput control.

Alternatives considered and not chosen: reporting the 9-cell ratio as a pass (it is the artifact D4
exists to prevent); flipping `cp_negotiation_enabled` to make the measurement actionable (F3 fails,
and it would mislead operators); and adding an `hf_robust_short_cp()` profile to remove the
base-profile confound (a new wire-visible codepoint for a profile `select_ofdm_profile` can never
select — the bench-local synthetic twin answers the same question without touching the spec).

Scope of evidence: simulated, single-transceiver, one frame-indexed schedule, five seeds decided on
sign consistency rather than significance — a **directional** result, following COP-3's "diagnostic
rather than statistical acceptance thresholds" precedent. Pairing is exact only at the channel level:
once delivery outcomes diverge, `RateLoop`'s trajectory diverges too, so the arms do not transmit the
same levels on the same frames. **There is no live two-radio validation**, and the levels ≥ 5 share of
the CP saving is an upper bound production cannot collect.

