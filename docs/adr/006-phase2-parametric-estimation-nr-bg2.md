# ADR-006: Phase 2 — parametric channel estimation + NR BG2 mother code

## Status

Accepted

See also: ADR-005 (the NR BG2 mother-code decision in isolation — decisions 5-7 below
cross-reference it rather than duplicate it). This ADR is the phase-level record: all 10 of
Phase 2's locked design decisions, where the shipped implementation deviated from the original
plan text, and the phase-closing cumulative re-baseline (Task 8) against the Phase 1 exit
baseline (`results/p1-hotfix/*.csv`).

## Context

Phase 1 ("radio reality") got Coppa's waveform into a realistic SSB passband with CFO tolerance
and fixed a sparse-pilot Watterson-fading regression (ADR-003, ADR-004). Coppa's "world-class"
and "deep review" analyses (see this repo's project memory) then identified Phase 2's target: a
cluster of related dB-harvest opportunities in channel estimation, header protection, LLR
calibration, and the LDPC mother code, estimated at roughly 5 dB of achievable gain in aggregate.
Phase 2 (seven dev tasks, `feature/db-harvest`) implements ten locked design decisions from that
plan; this task (Task 8) is the phase gate: full re-baseline, docs, and an honest accounting of
what was and was not achieved.

**Headline finding, stated up front:** three of Phase 2's seven tasks (Task 1's delay-domain
estimator, Task 7's Kalman tracker, and Task 4's NR BG2 LDPC) did not meet their own individual
acceptance bars, and two of those (Tasks 1/7) left a **regression** on Watterson-Moderate/level 2
that is not fixed by anything shipped later in the phase. Three tasks (2, 3, 5) delivered clean,
verified wins, and one (Task 6) delivered a partial win. The cumulative full-ladder re-baseline in
this ADR's "Consequences" section and in `BENCHMARKS.md`'s new Phase 2 section reports the net
effect honestly, including where it falls short of Phase 2's own stated acceptance bar. This is
consistent with — not an exception to — this project's established practice across every
DONE_WITH_CONCERNS task in this phase: measure rigorously, report exactly what is found, and let
the humans decide what to do with a shortfall.

## Decision

The plan locked ten design decisions. Each is recorded below as originally decided, followed by
where the shipped implementation deviated and why (all deviations were reviewed and accepted as
honest, well-justified engineering judgment, not scope-cutting).

### 1. Estimator: delay-domain ridge LS on the pooled pilot comb (Task 1)

**Plan**: model `H(k) = Σ_ℓ h_ℓ·e^(-j2πkℓ/Nc)`, `L ∈ 2..8` selected from the probe symbol, ridge
regularization `λ = σ̂²`, replacing `LinearInterpolationEstimator` in both the payload and header
demod passes.

**Shipped**: exactly this (`crates/coppa-codec/src/ofdm/delay_domain.rs`,
`DelayDomainEstimator`), wired via a shared `estimate_and_equalize` helper used by both
`demodulate_frame` and `demodulate_header_llrs`'s non-Kalman fallback path.

**Deviations, found and fixed along the way**:
- A one-time, construction-time **coarse bulk-delay self-calibration** (`measure_bulk_bias`,
  computed once per `CoppaModem::new()` on a clean channel) was added after wiring the estimator
  broke clean-channel loopback tests: `SyncDetector`'s CP-tolerant timing lock combined with
  `hf_standard`'s TX bandpass FIR left a deterministic ~1.5-grid-unit non-integer residual delay
  that, expressed in the model's integer-grid tap basis, spread across most available taps and
  inflated `noise_var` into the hundreds even with zero real channel noise. An *adaptive*
  (per-frame) version of this bias was tried first and measured to regress a header-survival test
  from 30/30 to 22/30 (ITU-R F.1487's two independently-fading Watterson taps can put the momentary
  "average" bias at a negative relative delay the model can't represent) — the fixed,
  construction-time version was kept instead.
- A **probe-vs-pooled degrees-of-freedom clamp** (`l = pooled.len().saturating_sub(2).clamp(2, 8)`)
  was added after discovering `select_order`'s probe-derived tap count (correctly sized for the
  full 48-carrier probe) left as few as 0 degrees of freedom when applied to `hf_standard`'s much
  sparser per-symbol pooled-pilot comb.
- **Unresolved, accepted regression**: even with both fixes, Watterson-Moderate/level 2 regressed
  from an 18 dB to a 24 dB FER≤10% threshold (needed ≥1.5 dB *better*) — root-caused to the
  estimator's frame-global `coarse_delay` not tracking real intra-frame drift (verified via
  per-window instrumentation showing a clean, monotonic |Ĥ|² decay across a frame, not fading
  noise). A per-window adaptive re-derivation was built, tested, and reverted: it improved
  hard-decision channel-estimate accuracy but made full-sweep FER measurably *worse*
  (0.095→0.2675 at 18 dB) because individual low-SNR windows occasionally produced wrong local
  estimates that corrupted the LDPC-facing `noise_var` (observed maxima in the billions). This is
  the regression Task 7 (decision 3) was pulled forward specifically to try to close.

### 2. Equalization stays one-tap ZF (Task 1)

**Plan**: `X̂ = Y/Ĥ`, per-carrier noise `σ²/|Ĥ|²`, `ε = 1e-4` erasure guard.

**Shipped**: exactly this, unchanged from Phase 1. No deviation.

### 3. Time tracking: boxcar pooling baseline; Kalman tracker as an optional stretch (Task 1 + Task 7)

**Plan**: the existing ±2-symbol boxcar pooling (`EST_WINDOW`) is the Phase 2 baseline. A Kalman/
RTS smoother was specified as an *optional* stretch task ("land only if under budget").

**Deviation — pulled forward, not optional**: because Task 1 left Watterson-Moderate/level 2
regressed rather than improved, the Kalman tracker (Task 7) was explicitly pulled forward and run
immediately, specifically to try to close that gap before the estimator went live on any
HF-profile path. **It also did not meet its own acceptance bar.** Task 7 built a genuine, correct
AR(1) Kalman filter (`crates/coppa-codec/src/ofdm/kalman_tracker.rs`, `KalmanLagSmoother` +
`TrackedTaps`) with a lag-2 RTS smoother, found and fixed one real bug along the way (consecutive
Kalman steps were being fed the same overlapping `±2`-symbol pooled pilots as independent
evidence, causing posterior confidence to compound far beyond its real information content — fixed
by feeding raw, non-overlapping single-symbol pilots per step instead), and verified the RTS
smoothing pass genuinely reduces error (not just latency) via a direct causal-vs-smoothed
comparison test. Despite the fix, a systematic sweep of the AR(1) forgetting coefficient
`a ∈ {0.5..0.99}` showed near-total flatness in FER response — the tracker's own tuning lever is
not the bottleneck — and the final measured Watterson-Moderate/level 2 FER≤10% threshold is **30
dB**, worse than both the pre-Phase-2 baseline (18 dB) and Task 1's own already-regressed 24 dB.
Task 7's report's working hypothesis is that the drift Task 1 documented is more consistent with a
continuously-accumulating phase/coarse-delay reference error than genuine stationary Rayleigh
amplitude fading, making a zero-mean mean-reverting AR(1) model a poor fit for the actual
mechanism — a model-class mismatch, not a tuning problem. **This regression is shipped, accepted,
and unresolved** — see "Consequences" and CLAUDE.md's Known Limitations.

An addendum to Task 7's report also flags a plausible *additional* contributor never
independently confirmed: `TrackedTaps::noise_at` returns the tracker's posterior tap-variance
(`Var(Ĥ(k))`), not a genuine observation-noise estimate — once the tracker's posterior is
confident, this can under-report the true noise floor and feed the LDPC decoder overconfident
LLRs. Task 5 (decision 10) explicitly does not use this quantity for its own re-fit (it uses
`DelayDomainEstimator::noise_var` instead, which is a genuine residual variance), so this
hypothesis remains open for whoever revisits Task 1/7's estimator next.

### 4. Header goes soft (Task 2)

**Plan**: `demodulate_header` returns per-bit LLRs; a new `decode_header_soft` performs full
4096-codeword correlation ML decoding with CRC-assisted list-2 rescue.

**Shipped**: exactly this (`crates/coppa-codec/src/ofdm/golay.rs`'s `golay24_decode_soft`,
`header_fec.rs`'s `decode_header_soft`), a clean, verified win — see "Consequences".

**Deviation**: the plan's acceptance figure ("≥25 percentage points header-decode-rate gain,
soft vs. hard, at 200 seeds of watterson-poor, 8 dB") was not achievable and was honestly
re-derived, not quietly relabeled. At 8 dB, hard decode was already 93-95% (soft 100%) — only
5-7 points of headroom existed at all, because Task 1/7's estimation improvements (already merged
onto this branch before Task 2 started) had already made hard-decision header decoding fairly
robust at that operating point. A systematic SNR/profile/frame-length sweep found the real,
reproducible gap is **6-8 percentage points**, at a lower SNR (~3 dB) with a shorter frame, before
sync itself (not FEC) becomes the binding constraint. The shipped acceptance test's threshold
(`≥5.0` points) reflects this measured reality with margin, reviewed and approved.

### 5. LDPC becomes one mother code: 5G NR BG2 (Task 4)

**Plan**: replace nine per-rate 802.11-Annex-F QC-LDPC codes with a single NR BG2 mother code,
`Zc=176`, `n_mother=8800`, `RV0/k0=0` for Phase 2, `k_used = round(rate × 1944)`, with level 10's
rate moving from 7/8 to 5/6 (a wire-format break).

**Shipped**: exactly this — see ADR-005 for the full decision record (base-graph transcription,
shortening/`k_used` table, circular-buffer rate matching, layered decoder). ADR-005 is not
duplicated here; **two shortfalls from ADR-005's own acceptance targets are carried forward
honestly**: level-2 AWGN isolated-FEC-layer coding gain measured **+0.5 dB** (target ≥1.2 dB,
believed to be a genuine finite-length effect against an asymptotic density-evolution prediction,
not a decoder-schedule bug — ruled out via a direct layered-vs-flooding A/B), and decode CPU/frame
measured **3.5x-9.5x** the old codec across the ladder (target ≤3x, because the shared mother
code's graph no longer shrinks for high-rate levels the way the old per-rate graphs did — a real,
verified ~19% optimization was found and shipped but did not close the gap; a follow-up syndrome-
check optimization was also implemented, verified correct, but measured **no further CPU
reduction**, reported honestly per that follow-up's own instruction not to force an appearance of
improvement).

### 6. BG2 tables via a checked-in generator-validator (Task 4)

**Plan**: `tools/gen_nr_bg2` transcribes 3GPP TS 38.212 Table 5.3.2-3 (BG2) with four structural
validators.

**Shipped**: exactly this, with one correction to the plan's own text found during
implementation: the plan guessed lifting-size family index **i_LS=7** for Zc=176; the correct
value is **i_LS=5** (index 7's family is `{15,30,60,120,240}`, which doesn't even contain 176).
Cross-validated against two independent open-source implementations (NVIDIA Sionna, srsRAN
Project), fetched fresh and diffed programmatically — all 197 non-zero entries matched exactly.
See ADR-005 for the full validator run.

### 7. Decoder: layered normalized min-sum, α calibration (Task 4)

**Plan**: layered (row-based) NMS decoding, α=0.8 initially, with a calibration sweep.

**Shipped**: layered NMS decoding as planned. **A real correctness bug was found and fixed
during Task 4's own Step 5 investigation**: α=0.80 measurably beat α=0.75 in a single-level
(level 2 only) calibration sweep and was initially shipped as the default — but this broke real
LDPC convergence at level 10 (100% frame loss, including on a clean channel), caught by
`tests/phase_c_loopback.rs`'s pre-existing tests, not by the new codec's own unit tests. **Shipped
default: α=0.75**, validated across the whole ladder. This is recorded in CLAUDE.md as a
standing lesson: validate any future LDPC-parameter change across the whole speed ladder and
payload-size extremes, not one representative level. A follow-up hardened this: a dedicated
regression test (`perfect_llr_loopback_level_10_tiny_payload`) now runs this exact scenario under
`cargo test -p coppa-protocol --lib`, which CI does execute (unlike the integration test that
originally caught it).

### 8. LLR calibration end-to-end (Task 3)

**Plan**: exact max-log LLR scales (BPSK `4·re/σ²`, QPSK `2√2·(re,im)/σ²`), replacing magic
noise-variance fallback constants with `median(noise_vars)`.

**Shipped**: exactly this, resolving a real factor-of-2 discrepancy between the header path
(already `4·re/σ²` since Task 2) and the pre-Task-3 payload path (`2·re/σ²`). **A clean, verified
+3.0 dB gain at the FEC-isolated level** (see "Consequences"), masked at the full end-to-end
bench level for an unrelated reason: a dedicated investigation (following this project's "never
trust a bench comparison without direct verification" practice) found the brief's specified
end-to-end bench produced byte-identical FER curves before and after, root-caused via direct
instrumentation to every measured frame failure at that operating point being an OFDM-sync
failure, never an LDPC-non-convergence failure — the pinning/calibration gain is real but
invisible until sync's own SNR floor is at or below the LDPC decode's floor. This masking
relationship is itself flagged as a fact worth knowing for future payload-FEC work, not a defect
in this task.

### 9. Known-pad pinning (Task 3, extended by Task 4)

**Plan**: pin `payload_len·8..k_used` to `±64·sign` after deinterleave, before decode.

**Shipped**: exactly this in Task 3 (`prbs_bits`, wired into `CoppaTransceiver::receive`), then
**extended in Task 4** to also pin the shortened tail (`k_used..1760`, the mother code's
never-transmitted region that `rate_dematch` otherwise leaves at `0.0`) in the same pass, since
both regions' pin values depend only on the PRBS keystream. No deviation from the plan beyond
this natural extension, itself already anticipated by the plan's own mother-code decision.

### 10. Turbo re-estimation: one round on decode failure (Task 5)

**Plan**: on LDPC non-convergence, build soft "virtual pilot" symbols from posterior LLR means,
weighted ridge refit, re-equalize/re-demap/decode once more.

**Shipped**: exactly this mechanism (`CoppaTransceiver::receive_with_metrics`'s retry path,
`CoppaModem::reequalize_with_virtual_pilots`), a clean, verified win concentrated on lower-order
modulation — see "Consequences".

**Deviation, an acknowledged plan gap, not a bug**: the plan's own sketch for the soft-symbol
closed forms (`e_mag = 2.0 - p0`) was an intentionally incomplete placeholder. The actual closed
forms were independently re-derived from this codec's real Gray-mapping tables: 16-QAM's exact
identity is `level = 2·tanh(l_msb/2) + tanh(l_lsb/2)` (the plan's placeholder does not match this);
64-QAM's is `level = tanh(l0/2)·(4 + 2·tanh(l1/2) + tanh(l1/2)·tanh(l2/2))`. Both were verified
against an independent brute-force reference to `<1e-5`. **8-PSK has no closed form** — its
constant-modulus circular geometry doesn't decompose into independent per-axis terms the way a
rectangular QAM grid does — so it correctly falls back to exact brute-force enumeration over all
`2^bits_per_symbol` points (still exact under the independence assumption, just O(2^bps) instead
of O(1), and only ever evaluated on the rare turbo-retry path). This is a plan deviation, not a
bug: the plan's own sketch did not anticipate this geometric distinction.

## Related, adjacent work not itself one of the 10 decisions

**Task 6 (fast Gray-QAM demappers)** was a performance task with no dB target, included in this
phase's scope but not one of the 10 locked decisions above. It replaced `demap_soft`'s O(bits ×
levels) brute-force enumeration with closed-form per-axis min-reduction arithmetic for 16-QAM and
64-QAM. 64-QAM clears its ≥8x speedup target on the full production API (28-34x measured); 16-QAM's
full-API measurement falls short (4.2-4.3x) for a well-diagnosed, out-of-scope structural reason —
the underlying closed-form arithmetic itself is 19-20x faster, but both the old and new code paths
pay an identical, fixed `Vec<f32>` heap-allocation cost (via the shared `ConstellationMapper`
trait) that dominates at 16-QAM's smaller workload (4 bits × 16 candidate points) and would require
a cross-cutting trait-signature change (affecting `bpsk.rs`/`qpsk.rs`/`psk8.rs` and every
downstream consumer) to remove.

## Consequences

### Cumulative full-ladder re-baseline (Task 8, this ADR's own measurement)

400 trials/point, `-6..30` dB step 3 dB, all measurable speed levels (1-7, 9, 10 — level 8 is
reserved 32-QAM and excluded, matching every prior gate in this codebase), against
`results/p1-hotfix/{awgn,moderate,poor}.csv` (the Phase 1 exit baseline). Full tables are in
`BENCHMARKS.md`'s new Phase 2 section (`results/p2-final/`, `results/p2-final-ssb/`,
`results/p2-final-cfo40/`); the summary here is the headline finding.

**AWGN**: met and exceeded. Level 4 gains +3 dB, level 7 +3 dB, level 9 +6 dB at FER≤10%; level
10's rate-7/8→5/6 change fixes the pre-Phase-2 non-convergence entirely (peak goodput
399→8450 bps). Two small, real exceptions: levels 6 and 9 each show their FER≤1% threshold get
3 dB worse or undefined (a residual ~1-1.25% error floor), consistent with Task 4's own finding
that the new mother code's coding gain is real but modest at matched block length.

**Watterson Good** (no Phase-1-exit baseline; compared to `results/p1-final/good.csv`): a clean
win — levels 1-2 improve 9-15 dB at FER≤10%, most other levels show real goodput gains, level 6
is bit-for-bit unchanged.

**Watterson Moderate**: a genuinely mixed result. Levels 1-2 (BPSK) improve +6 dB / +3 dB at
FER≤10% — turbo re-estimation's BPSK-concentrated rescue (decision 10) outweighing the
estimator regression (decisions 1/3) there. **Levels 3-6 (QPSK, 8PSK, 16QAM 1/2) show a real
regression at matched operating SNR** — not just an unmet metric, FER is measurably worse (e.g.
level 6 at 30 dB: 10.25%→40.0%). Levels 7 and 9 (16QAM 3/4, 64QAM 2/3) improve consistently at
every SNR point, matching the AWGN ladder's pattern of the new LDPC's largest gains landing on
the previously-weakest, highest-rate levels.

**Watterson Poor** (the phase's own named acceptance channel): the FER≤10% threshold never
crosses for any level, before or after — an irreducible outage floor at this trial count,
matching every prior Task 1/5/7 finding. The underlying FER curves show level 1-4 (BPSK/QPSK)
substantially better at every SNR (level 2 at 30 dB: 44.25%→21.75%, roughly halved); levels 5-6
(8PSK, 16QAM 1/2) flat to marginally worse; levels 7/9 clearly better; level 10 still
non-functional.

**`--ssb`/`--cfo 40`**: the SSB sweep matches the AWGN ladder's pattern closely (no new
Phase-2-specific effect). The CFO sweep surfaces a **new regression not visible in plain AWGN**:
level 4 (QPSK 3/4) goes from clearing FER≤10%/≤1% at 15/18 dB to never clearing at all under a
40 Hz offset (peak goodput 1849→994 bps) — not exercised by any single dev task's own bench
gate, flagged as a follow-up.

**Header failure share on Poor**: met in aggregate (4.3% across levels 2/3/6, all SNR points),
briefly exceeding 10% only at the two lowest SNR points tested (6/12 dB, where the payload
itself is already failing 30-60% of the time); from 18 dB up every cell is ≤4%.

### Phase-acceptance bar (as stated in the Phase 2 plan)

The plan's own acceptance bar: cumulative vs. Phase 1 exit, ≥+3 dB at FER@10%-CI on
watterson-poor/level 2, ≥+1.5 dB at level 6; AWGN ladder ≥+1 dB from the code swap; header
failures <10% of residual frame failures on poor (soft Golay).

| Criterion | Result |
|---|---|
| AWGN ≥ +1 dB | **Met and exceeded** |
| Watterson-poor/level 2 ≥ +3 dB at FER@10%-CI | **Not measurable** as literally specified (neither codec ever crosses 10% FER on Poor at level 2); the underlying FER is substantially better at every SNR (a real, verified gain the literal metric can't express) |
| Watterson-poor/level 6 ≥ +1.5 dB at FER@10%-CI | **Not met** — not measurable by the literal metric, and unlike level 2 the underlying FER shows no net improvement |
| Header failures < 10% of residual on poor | **Met in aggregate** (4.3%), with a low-SNR caveat |

**Net: the phase's own acceptance bar is not cleanly met.** See `BENCHMARKS.md`'s Phase 2
section for the complete per-level, per-channel tables this summary condenses.

### Why a mixed/shortfall result here is expected, not a surprise

Given that Tasks 1 and 7 (channel estimation) left a real, unresolved *regression* on
Watterson-Moderate/level 2, and Task 4 (LDPC) fell short of its own coding-gain and CPU targets,
it would have been a surprise if the cumulative full-ladder measurement cleanly cleared the
phase's own acceptance bar everywhere. The honest picture is a genuine mixed bag: Tasks 2, 3, and
5's real, verified wins (soft header, LLR calibration, turbo re-estimation) partially offset
Tasks 1/7's regression and Task 4's shortfall at some levels/channels, and do not at others. This
is reported in full in `BENCHMARKS.md`, not adjusted or hidden.

### Wire-format break

Frames encoded by this phase's codec (NR BG2 LDPC, protected soft header, level 10's 5/6 rate) are
not decodable by any pre-Phase-2 codec, and vice versa — this compounds Phase 1's own waveform
break (ADR-003) and Task 4's own LDPC break (ADR-005) into a single Phase 2 wire-format
generation. This is acceptable pre-1.0 (no deployed installed base).

## Related

- `docs/adr/003-phase1-waveform-break.md`, `docs/adr/004-strongest-path-timing.md` — Phase 1's
  own wire-format break and its fading-regression hotfix.
- `docs/adr/005-nr-bg2-ldpc.md` — the NR BG2 LDPC decision in full detail (decisions 5-7 above).
- `.superpowers/sdd/p2-task-{1,2,3,4,5,6,7}-report.md` — full per-task investigation detail this
  ADR summarizes.
- `.superpowers/sdd/p2-task-8-report.md` — this task's full report, including the complete
  cumulative re-baseline data.
- `BENCHMARKS.md`'s Phase 2 section — full before/after tables.
