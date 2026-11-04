# COP-134: causes and fix plan for the fading regressions

Date: 2026-11-04. Ticket: COP-134, H-108 from COP-87.
Measured baseline: `d837c89d388eaf9c70752c72db8537d910094cc8`.
Status: historical causal contributors isolated; current-head remedies remain unverified.
All measurements below are simulations. No RF trial was run.

The level-4 residual has a demonstrated timing/estimation contributor: the
30-sample timing backoff adds a phase ramp that sparse-pilot linear interpolation
handles poorly. The levels 3 to 6 Moderate regression starts with the Phase-2
delay-domain estimator/equalizer replacement, before the NR FEC change. Paired
receiver ablations below establish these causes on historical revisions. Both
limitations remain open because no remedy has been validated on current head.

## Historical evidence

These are two comparisons at different stages of the implementation. They do
not measure a single isolated receiver change.

| Gap | Historical observation | Tracked evidence |
|---|---|---|
| Level 4 after the Phase 1 timing hotfix | Moderate peak goodput was 1632 bps before Phase 1, 633 bps broken, then 1234 bps after the hotfix. Poor was 772, 333, then 555 bps. Recovery was only 72% to 76% of the pre-Phase-1 peak. | [BENCHMARKS, hotfix section](../../BENCHMARKS.md#2026-07--hotfix-sparse-pilot-header-watterson-fading-regression-adr-004), [ADR-004](../adr/004-strongest-path-timing.md) |
| Levels 3 to 6 after cumulative Phase 2 | Moderate FER regressed at matched SNR. Level 6 at 30 dB went from 10.25% to 40.0% FER. | [ADR-006, cumulative results](../adr/006-phase2-parametric-estimation-nr-bg2.md#consequences), [limitations](../LIMITATIONS.md) |

The hotfix tables used 400 trials per point with the same seed/grid as their
historical comparator. Peak goodput is the maximum over each cited sweep, not
necessarily a matched-SNR point. Phase 2 changed estimation, tracking and coding
among other components. Chronology alone cannot assign the Moderate regression
to the estimator or the NR code. These historical figures are not estimates of
current-head performance after later receiver and benchmark corrections.

[ADR-009](../adr/009-cop24-speed-ladder-renumber.md) leaves levels 1 to 7
unchanged. Old level 9 is current 8, and old 10 is current 9. Thus these level-4
and level-3-to-6 numbers need no renumbering, but profile, waveform and payload
still have to match for a valid comparison. Level 3 is QPSK 1/2, not BPSK;
level 4 is QPSK 3/4.

## What is established, and what is not

- The earlier Phase 1 header failure has a demonstrated cause. Bisection in
  ADR-004 isolated first-arriving rather than strongest-path sync anchoring.
  Preferring the strongest peak within the half-CP safety bound recovered most
  of that loss. The residual level-4 loss survived that fix.
- The earlier QPSK-sensitivity hypothesis was not isolated in the hotfix report
  or ADR-004. The new timing/ramp ablations below supply direct evidence of a
  receiver contributor. They do not establish an inherent QPSK limitation.
- The separate 40 Hz CFO failure at level 4 was traced to sync-timing jitter
  corrupting the delay-domain reference. The bounded coarse-delay correction
  fixed it. [ADR-006's addendum](../adr/006-phase2-parametric-estimation-nr-bg2.md)
  records the fix and combined CFO/fading checks. Those checks do not establish
  recovery of either historical fading curve.
- The later [fading root-cause investigation](../../BENCHMARKS.md#2026-07--fading-root-cause-investigation-the-coarse-delay-reference-theory-falsified)
  falsified the coarse-delay-reference explanation at level 2. Moderate's
  configured 0.5 Hz Doppler spread, Gaussian PSD sigma 0.25 Hz, leaves about 10%
  correlation by the end of a 1.365 s level-2 frame. Single-tap FER at 9/12/15 dB
  was 23.25/12.75/3.50%, versus 15.00/10.75/5.25% for two taps. Delay estimates
  did not stabilize or predict decode outcome on fading frames. This supports
  a within-frame fading/outage contribution there. It does not isolate what
  changed at levels 3 to 6 or explain the residual level-4 hotfix comparison.
- Later HARQ trial eviction and receiver observation-noise fixes changed the
  measurements, as recorded in [LIMITATIONS](../LIMITATIONS.md). Independent
  synthetic payloads must not share failed HARQ LLR state. An old/current
  comparison that changes this trial behavior is confounded.
- [COP-118](COP-118-fade-diversity.md) measured adapted level-2 session survival.
  Its outer-code prototype completed 5/5 Moderate sessions against 1/5 matched
  controls, at substantial clean-channel airtime and latency cost. Its adapted
  channel, ACK assumptions and receiver path differ from the frame sweep.
  It is evidence for a candidate mechanism, not a demonstrated fix for these
  frame regressions. Short CP likewise has channel/SNR-dependent results in
  [BENCHMARKS](../../BENCHMARKS.md#2026-07--short-cp-coherence-time-lever-measured-against-real-fading).

Searches of the available read-only research/plans history by COP-134, H-108,
`fade-spike` and `repository: coppa` found no separate fade-spike report. Its
backlog overlap therefore cannot be adjudicated here. This is an evidence
limit, not proof that no such report exists elsewhere.

## Current smoke evidence

The planning phase measured the revision named above with this command:

```sh
cargo run -q -p coppa-bench --release -- --channel moderate --level 4 --snr-min 30 --snr-max 30 --trials 20 --seed 12648430 --out-dir /tmp/cop134-plan-bench
```

The output directory is a disposable destination, not a durable source. The
measurement is preserved here. The default per-level profile and one-codeword
payload apply, with no added CFO or SSB filter. The runner references noise to
clean pre-fade signal power in a 3 kHz bandwidth, so fades reduce receive SNR.

| Measurement | Channel | Level | SNR | Errors/trials | FER, 95% Wilson interval | Goodput | Failures |
|---|---|---|---|---|---|---|---|
| Planning smoke at `d837c89d388eaf9c70752c72db8537d910094cc8` | Moderate | 4, QPSK 3/4 | 30 dB | 11/20 | 55%, 34.21% to 74.18% | 813.71 bps | 2 sync, 9 LDPC |

Implementation reran the same command with only the output directory changed to
`/tmp/cop134-implement-bench`, on the same measured revision. It reproduced the
planning result. The reproduced CSV row, including its header, is:

```csv
level,mode,channel,snr_db,trials,frame_errors,fer,fer_lo,fer_hi,ber,goodput_bps,sync_failed,header_corrupt,ldpc_not_converged,crc_mismatch,wrong_payload
4,QPSK 3/4,watterson-moderate,30.0,20,11,0.550000,0.342085,0.741802,0.00000000,813.71,2,0,9,0,0
```

FER means failed frames divided by attempted frames. Goodput is delivered
payload bits per simulated transmit second, `payload_bits * (1 - FER) /
frame_airtime_s`. It excludes session ACK and retry overhead. Definitions and
CSV serialization are in [runner.rs](../../crates/coppa-bench/src/runner.rs),
[metrics.rs](../../crates/coppa-bench/src/metrics.rs) and
[report.rs](../../crates/coppa-bench/src/report.rs).

This small simulation sample shows failures at one current operating point.
It is not a historical before/after reproduction, a full-sweep peak, or an
operating guarantee. In particular, 813.71 bps cannot be compared directly
against the historical peak goodputs above.

## Causal diagnosis on recovered historical revisions

Remediation ran these experiments on 2026-11-04. Historical source objects were
recovered by deepening the ticket branch's history. Each build used an isolated
archive, outside the working checkout. No product source was changed for this
report. All source mtimes were refreshed between revisions so Cargo rebuilt each
changed library. The retained runs have 400 trials per point. Earlier exploratory
100-trial runs are excluded.

All rows in this section use 30 dB, default profiles, no added CFO or SSB, and
seed `51552255982`, which is `0x00C0FFEE + (12 << 32)`. This reproduces the
30 dB seed position in the historical -6 to 30 dB, 3 dB-step sweep. Payloads,
channel seeds and noise seeds follow each revision's native runner. Levels 3/4
use HF standard; levels 5/6 use VHF wide. These revisions precede persistent
IR-HARQ combining, so failed trials cannot contaminate subsequent trials.
The experiments do not compare historical receivers with current HARQ behavior.

### Level 4: timing backoff makes sparse-pilot interpolation fail

The post-hotfix receiver at `50e385f` still subtracts `TIMING_BACKOFF = 30`
samples from its strongest-path timing anchor. An earlier FFT window adds a
frequency-dependent phase ramp to the channel that the linear complex pilot
interpolator must approximate. Across six active-carrier spacings, the backoff
alone adds `2*pi*6*30/960 = 1.178` radians of phase change. Interpolating complex
values along chords then changes both estimated amplitude and phase between
pilots. The response already contains the two-tap fading ripple.

This is a receiver interaction, not evidence that QPSK itself is defective.
The same transmitted samples, channel and noise were decoded with only the
backoff changed. A second ablation kept the timing window unchanged and removed
its known ramp before the existing interpolation/equalization in both header
and payload passes. It multiplies pooled received pilots and received carriers
at active index `k` by `exp(+j*2*pi*k*30/960)`. Known pilot values stay unchanged.
The equalizer removes the common rotation. This second intervention isolates
the interpolation coordinate system from window placement.

| Level-4 receiver at `50e385f` | Channel | Errors/400 | FER, 95% Wilson interval | Goodput bps |
|---|---|---|---|---|
| Native 30-sample backoff | Moderate | 154 | 38.50%, 33.86% to 43.36% | 1137.07 |
| Zero backoff | Moderate | 39 | 9.75%, 7.21% to 13.05% | 1668.62 |
| 60-sample backoff | Moderate | 396 | 99.00%, 97.46% to 99.61% | 18.49 |
| Native backoff with ramp compensation | Moderate | 33 | 8.25%, 5.93% to 11.36% | 1696.36 |
| Native 30-sample backoff | Poor | 287 | 71.75%, 67.15% to 75.94% | 522.31 |
| Zero backoff | Poor | 236 | 59.00%, 54.12% to 63.71% | 758.04 |
| Native 30-sample backoff | AWGN | 0 | 0%, 0% to 0.95% | 1848.89 |
| Zero backoff | AWGN | 0 | 0%, 0% to 0.95% | 1848.89 |

On Moderate, zero backoff recovers 123 native failures and loses 8 native
successes. Ramp compensation recovers 127 and loses 6. These paired counts
separate the intervention effects from independent-sample variation.

The recovered pre-Phase-1 revision `b5b4c01` independently measured 43/400
Moderate errors, 10.75%, interval 8.08% to 14.17%, and 1650.13 bps at this point.
That waveform differs, so it is a contextual comparator, not the paired causal
control. Neither its goodput nor the ablation rows replace the published sweep
peaks. The paired interventions on `50e385f` identify a contributor large enough
to explain the residual loss at this operating point. They do not establish
full-curve recovery, eliminate Poor outages, or license changing today's timing
safety policy. Compensation is the preferred candidate for a fix investigation
because it retains the timing margin; its Poor and full-ladder controls remain
unmeasured.

### Phase 2: the estimator/equalizer replacement introduces the loss

A chronological comparison at the same 400 seeds locates the large loss before
the FEC replacement. Entries below are failed frames out of 400.

| Revision | Change completed | Level 3 | Level 4 | Level 5 | Level 6 |
|---|---|---|---|---|---|
| `50e385f` | Phase 1 hotfix | 32 | 154 | 101 | 41 |
| `3c644c4` | Delay-domain estimator | 75 | 166 | 205 | 183 |
| `6dd6793` | Kalman payload tracker | 90 | 187 | 204 | 167 |
| `2adab69` | Soft header and LLR/pad work | 95 | 189 | 204 | 166 |
| `240acc0` | NR BG2 FEC | 75 | 179 | 188 | 163 |
| `c230da1` | Cumulative Phase 2 including turbo | 69 | 166 | 181 | 160 |

The level-6 endpoints exactly reproduce the documented 10.25% to 40% regression
at 30 dB. Native Phase-1-to-Phase-2 FER intervals are respectively 7.65% to
13.61% and 35.32% to 44.87%. Later coding improves some rows but does not remove
the loss introduced at the estimator boundary.

To distinguish causation from chronology, a receiver-only ablation at `3c644c4`
replaced the output of `estimate_and_equalize` with the previous linear
interpolator's zero-forced symbols and per-carrier noise. It retains the new
pilot extraction/pooling, transmitted waveform, payload, seeds and FEC. The
replacement returns linear noise divided by `mean_weight(pooled)` because the
new caller multiplies by that factor. This preserves the old noise contract.
The helper is shared by header and payload, so the intervention covers both.

| Receiver output at `3c644c4` | Level 3 | Level 4 | Level 5 | Level 6 |
|---|---|---|---|---|
| Native delay-domain symbols and noise | 75 | 166 | 205 | 183 |
| Linear symbols and linear noise | 32 | 154 | 101 | 41 |
| Delay-domain symbols with linear noise | 114 | 218 | 249 | 229 |
| Linear symbols with delay-domain noise | 44 | 142 | 192 | 124 |

Restoring both outputs reproduces every baseline trial outcome, not merely its
aggregate FER. Replacing noise alone worsens every row; replacing symbols alone
recovers only part of the loss at levels 5/6. This identifies the joint
estimation/equalization and soft-noise contract as the causal regression site.
A noise-only fix or a FEC-only explanation does not explain these results.
The constrained delay-grid model, probe-selected order and residual-derived
noise are coupled in this replacement. The experiments do not isolate one of
those numerical details as the sole defect. In particular, they do not revive
the falsified coarse-delay-drift explanation or attribute every failed frame
to an estimator error.

The supported fix direction is a fading-aware estimator/equalizer correction
validated against the historical linear-output counterfactual. A confidence-gated
fallback is a candidate, not a shipped remedy. Its current-head implementation
must account for the Kalman path, later observation-noise fixes and turbo
re-estimation. A wholesale estimator rollback would discard those changes.

### Evidence and reproduction

[The evidence CSV](COP-134-fading-evidence.csv) preserves revisions, intervention
names, FER intervals, goodput and paired per-trial outcomes. `success_bits_hex`
encodes successes with trial `t` in bit `t`, least significant bit first.
Each row has 400 trials, seed base `51552255982`, and SNR 30 dB. Unmodified
pre-Phase-1 rows lack bitsets because that run did not print trial outcomes.
The supplied script snippets describe temporary diagnostic edits, not proposed
product patches.

For each revision, extract `git archive <revision>` into a separate scratch
directory. Restrict the CLI loop to
`MODES.iter().filter(|m| (3..=6).contains(&m.level))`, or level 4 for timing
experiments. Before the return in `run_trial`, print
`eprintln!("TRIAL,{level},{seed},{}", outcome.success);`. Refresh all Rust source mtimes if
reusing a build directory, then run:

```sh
cargo run --release -p coppa-bench -- --channel moderate --snr-min 30 --snr-max 30 --trials 400 --seed 51552255982 --out-dir results/cop134
```

The timing experiment uses `50e385f`. Change only `TIMING_BACKOFF` to 0 or 60,
or keep 30 and apply this replacement at both `estimator.update(&combined)`
sites, borrowing the new `carriers` vector in the subsequent `mmse_equalize`:

```rust
let rot = |k: usize| Complex32::from_polar(
    1.0, 2.0 * std::f32::consts::PI * k as f32 * 30.0
        / self.profile.fft_size as f32,
);
let combined: Vec<_> = combined.iter()
    .map(|&(k, y, x)| (k, y * rot(k), x)).collect();
estimator.update(&combined);
let carriers: Vec<_> = carriers.iter().enumerate()
    .map(|(k, &y)| y * rot(k)).collect();
```

The estimator ablation uses `3c644c4`. Immediately before the original
`est.equalize(&rotated_carriers)` return in `estimate_and_equalize`, compute:

```rust
use super::equalizer::{LinearInterpolationEstimator, mmse_equalize};
use crate::traits::ChannelEstimator;
let observations: Vec<_> = pooled.iter()
    .map(|&(k, h, _)| (k, h, Complex32::new(1.0, 0.0))).collect();
let mut linear = LinearInterpolationEstimator::new(nc);
linear.update(&observations);
let indices: Vec<_> = (0..nc).collect();
let gains = linear.per_carrier_gain(&indices);
let mut linear_eq = mmse_equalize(carriers, &linear, nc);
for (y, g) in linear_eq.iter_mut().zip(gains) { *y /= g; }
let linear_noise: Vec<_> = linear.per_carrier_noise(&indices).iter()
    .map(|v| v / mean_weight(pooled)).collect();
let (delay_eq, delay_noise) = est.equalize(&rotated_carriers);
```

Return `(linear_eq, linear_noise)`, `(delay_eq, linear_noise)`, or
`(linear_eq, delay_noise)` for the three interventions. Repeat the native and
linear-output cases with `--channel awgn` as a high-SNR control. Warnings about
unused diagnostic locals are expected; no warnings were suppressed in product
code. The native and linear-output AWGN controls each decoded 400/400 frames at
every level from 3 through 6. A single high-SNR control is not a waterfall or
full-ladder safety check.

The historical component loss is now reproduced and causally localized. Remedy
transfer, additional SNRs/seeds, current-head full-ladder controls and RF behavior
remain the follow-up's work. The original 20-trial current smoke is still a
separate measurement, with its original provenance preserved above.


## Controlled diagnosis and fix plan

Choose the fix-plan route. Retain both open limitations. There is no acceptance
of the gaps as fixed and no change to product defaults or operating limits.
The historical ablations above complete the causal localization in this change.
The follow-up must transfer the candidates to current head and verify remedies;
the remaining experiments below do not imply that those remedies already work.

1. Recover the pre-Phase-1, hotfix, pre-Phase-2 and post-Phase-2 comparators.
   Record full revisions, profile/waveform, payload length/content, seeds and
   SNR grid/convention. First measure current head with corrected HARQ eviction.
   Recover historical trial counts and raw CSVs; preserve new per-trial results.
   If artifacts or compatible builds cannot be recovered, record each missing
   comparator and incompatibility. Do not call a current-only sweep a historical
   reproduction.
2. Use paired channel/noise draws and AWGN controls. Change one component at a
   time to isolate estimator/tracker, FEC and retry contributions at levels
   3 to 6, plus the level-4 residual comparison. Where old waveforms prevent
   common received samples, match channel traces, payload and power conventions
   and report the remaining mismatch. Separate independent-frame trials from
   real retransmissions and verify their HARQ eviction/combining semantics.
3. Count sync, header, LDPC, CRC and wrong-payload failures. Preserve channel
   truth, channel-estimate error and LLR calibration diagnostics per trial.
   Compare flat and two-tap fading, varied Doppler and frame duration. A proposed
   contributor needs a repeatable paired effect and a trial-semantics control;
   a correlation or failed-decoder label alone is insufficient.
4. Evaluate remedies only after demonstrating a repeatable contributor.
   Shorter airtime and diversity are candidates. Report 95% FER intervals,
   paired improvement uncertainty, goodput, airtime, latency, power and wire
   compatibility costs. Set trial counts and material-regression margins before
   candidate selection. Check the full speed ladder on AWGN, Good and Poor,
   alongside affected Moderate points. Reject a remedy that merely shifts
   failures or breaches those control margins.
5. Publish raw evidence and either the isolated cause with a verified remedy,
   or a bounded negative result. Update LIMITATIONS with the measured envelope
   and decision. Keep the gaps open if comparators or improvement evidence are
   insufficient. Retrieve any fade-spike report before duplicating experiments
   and record which parts it actually settles.

## Unfiled follow-up ticket draft

Title: Engineers should reduce fading losses so that speed-level reliability improves on measured channels

Context: The historical level-4 residual and levels 3 to 6 Moderate regression
have historical causal evidence above, but remain open pending current-head
remedy validation. This draft has not been filed and has no assigned ticket number.

```gherkin
Scenario: Engineers can compare the two historical fading gaps on matched settings.
  Given the recoverable pre-Phase-1, hotfix, pre-Phase-2 and post-Phase-2 revisions
  When engineers publish the matched level-4 and levels-3-to-6 fading comparisons
  Then the report identifies revisions, profiles, payloads, seeds and trial semantics
  And it includes per-trial outcomes, FER intervals, goodput and AWGN controls

Scenario: Engineers can see when a historical comparison cannot be recovered.
  Given a historical revision or measurement artifact cannot support matched trials
  When engineers publish the comparison report
  Then the report names the unavailable comparator and the demonstrated incompatibility
  And it labels current-only results separately and retains the unresolved limitation

Scenario: Engineers accept a remedy only after a contributor is isolated.
  Given paired ablations demonstrate a repeatable contributor to an affected fading gap
  When engineers evaluate a targeted remedy using predeclared trial counts and margins
  Then the report shows a paired FER improvement with uncertainty and its goodput costs
  And full-ladder AWGN, Good and Poor controls meet the declared regression margins
  And gaps without sufficient improvement evidence remain open in LIMITATIONS
```

### Technical notes

Evaluate timing-ramp compensation and an estimator/equalizer correction against
the historical counterfactuals above. Execute the remaining controlled plan.
Instrument failure categories and channel/LLR
quality before selecting a fix. Keep frame reliability and session survival
separate. The scope includes a negative-result path; it must not manufacture a
cause when ablations fail to isolate one.

### References

- [ADR-004](../adr/004-strongest-path-timing.md), the earlier timing cause and partial fix.
- [ADR-006](../adr/006-phase2-parametric-estimation-nr-bg2.md), cumulative Phase 2 and CFO evidence.
- [ADR-009](../adr/009-cop24-speed-ladder-renumber.md), historical level mapping.
- [BENCHMARKS](../../BENCHMARKS.md), historical sweeps and level-2 diagnostics.
- [COP-118](COP-118-fade-diversity.md), bounded session-diversity evidence.
- [LIMITATIONS](../LIMITATIONS.md), the open gaps this follow-up must update.
