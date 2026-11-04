# COP-134: evidence and diagnosis plan for the fading regressions

Date: 2026-11-04. Ticket: COP-134, H-108 from COP-87.
Measured baseline: `d837c89d388eaf9c70752c72db8537d910094cc8`.
Status: evidence review complete; root causes and remedies remain unverified.
All measurements below are simulations. No RF trial was run.

The level-4 residual fading gap and the levels 3 to 6 Moderate regression remain
unresolved. Existing evidence does not isolate their causes or verify a fix.
The strongest-path timing fix recovered most earlier losses. The separate
level-4 CFO failure was fixed. Neither result closes these fading gaps.

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
- QPSK sensitivity to residual channel-estimate errors is a hypothesis for the
  level-4 residual. Neither the hotfix report in BENCHMARKS nor ADR-004 isolates
  it by a matched ablation. It is not a proven diagnosis or a workaround with a
  measured reliability guarantee.
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

## Controlled diagnosis and fix plan

Choose the fix-plan route. Retain both open limitations. There is no acceptance
of the gaps as fixed and no change to product defaults or operating limits.
The following experiments belong to the follow-up, not this documentation change.

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

Title: Engineers should isolate fading regressions so that speed-level reliability rests on matched evidence

Context: The historical level-4 residual and levels 3 to 6 Moderate regression
remain open. This draft has not been filed and has no assigned ticket number.

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

Execute the controlled plan above. Instrument failure categories and channel/LLR
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
