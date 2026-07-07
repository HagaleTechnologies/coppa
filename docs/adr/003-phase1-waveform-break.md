# ADR-003: Phase 1 waveform break — carrier offset, Newman preamble, TX conditioning, streaming sync, two-stage CFO

## Status

Accepted

## Context

Coppa's waveform, as it existed through the `feature/hotfix-fec-correctness` baseline, occupied
the full Nyquist band starting at bin 1 (~50 Hz) and used a full-Nyquist PN-BPSK comb preamble.
That is not how any real HF SSB radio actually presents audio: a typical SSB transceiver passes
roughly 300–2700 Hz and heavily attenuates or rejects everything outside it (its own crystal
filter and AGC). A waveform that assumes energy at 50 Hz or above 2700 Hz will have that energy
attenuated or removed by the radio before it ever reaches the channel, and a receiver detection
scheme built and tuned against a full-Nyquist signal is not validated against what a real radio
actually delivers.

Phase 1 ("radio reality") closes this gap: it moves the waveform into a realistic SSB passband,
makes the preamble fit that passband, restores TX signal conditioning lost in the process, adds
an SSB channel emulation to the bench harness so the gap can be measured, replaces the batch
preamble search with a streaming O(1) detector suited to a real audio pipeline, and adds carrier
frequency offset (CFO) tolerance (a real SSB rig's dial and oscillator are never exactly on
frequency). Six tasks (1–6) implement this; two more (7–8) migrate the daemon/FFI to a streaming
receiver and gate the whole phase. This ADR records the five locked design decisions from Tasks
1–6, not the streaming-receiver plumbing (that is an application-layer consequence, not a
waveform decision).

## Decision

1. **Carrier offset.** `CoppaProfile` gained a `carrier_offset: usize` field (6 bins on all
   profiles), so active carriers start at bin 6 (~300 Hz) instead of bin 1 (~50 Hz). `hf_wide`'s
   data-carrier count dropped 50 → 46 to keep the top of the occupied band inside ~2700 Hz. This
   is a **wire-format break**: bin assignment is baked into the frame structure, so old and new
   waveforms are not interoperable.

2. **Newman-phase in-band preamble.** The preamble comb moved from full-Nyquist PN-BPSK to a
   Newman-phase comb confined to in-band even bins, unit-RMS normalized by construction. A
   full-Nyquist preamble literally cannot fit inside an SSB passband — energy outside 300–2700 Hz
   is simply not there on a real link, so a preamble that assumes it is unusable outside a clean
   loopback. Also a wire-format break (different preamble bits/energy pattern than before).

3. **TX conditioning chain** (per-section RMS leveling → RC-overlap taper → PAPR clip →
   601-tap HF-only bandpass → peak-normalize to 0.5 FS). This was added to fix a real regression
   that decisions 1–2 introduced and that their own task reviews did not catch (they checked spec
   compliance and re-derived the PAPR math, but never ran a full AWGN acceptance sweep against
   baseline): the unit-RMS-normalized preamble sits tens of dB hotter than the naturally quiet
   sparse-bin payload body, and since the bench's SNR convention references injected noise to the
   whole frame's mean power, that imbalance silently starved the payload of effective SNR (9+ dB
   loss at low speed levels, complete decode failure at every tested SNR for levels 5–10 — see
   BENCHMARKS.md's "2026-07 — Radio-reality Phase 1" section for the full measured regression and
   recovery). Per-section RMS leveling alone accounts for essentially the entire recovery.
   **Dead-end lesson recorded so it is not retried:** filtering a hard-clipped waveform regrows
   peaks via ringing at the clip corners (~5 dB for a single clip→filter pass), and iterating
   clip→filter a few times measurably improves RMS-at-fixed-peak in isolation (~4.9 dB by 3
   passes) but produced **no net FER improvement** in this codebase's bench harness — the bench's
   SNR convention references noise to the transmitted signal's own mean power, so a pure
   gain/peak-efficiency change is exactly self-cancelling under it. This would matter on real,
   peak-limited hardware with a channel noise floor independent of transmit power; it is invisible
   to (and was reverted from) this repo's own bench harness, so it was not implemented.

4. **RX bandpass + SSB channel model.** The RX path gained its own bandpass stage, and
   `coppa-channel` gained `ssb_filter` (a 601-tap FIR emulating a realistic 300–2700 Hz SSB rig
   audio passband) plus a `--ssb` bench flag, so the SSB-reality gap this phase exists to close
   can actually be measured rather than assumed.

5. **Streaming `SyncDetector` replacing batch search.** The old preamble search
   (`SchmidlCox`/`LtsCorrelator`/`detect_coppa_version` et al., all deleted) ran a batch
   correlation over a fixed buffer — workable for the old loopback-style API, but not for a real
   streaming audio pipeline that must process arbitrarily long, possibly silent, possibly
   tone-polluted input with bounded, non-buffer-length-proportional CPU. The new `SyncDetector`
   (`crates/coppa-codec/src/ofdm/sync_detector.rs`) is O(1) per pushed sample: a `DelayLine` +
   Hilbert `StreamingFir` maintain an analytic signal, a small ring maintains the
   `P`/`E1`/`E2` plateau-detection recurrence evaluated every `STRIDE=16` samples, and a
   confirm+first-path cross-correlation step (which also rejects steady tones and locks the
   first, not necessarily strongest, multipath arrival) resolves a candidate once its confirm
   window has arrived. Measured at 0.0015–0.0035x realtime on 10 s of noise (target ≤0.005x),
   comfortably meeting the CPU budget for a system that must run continuously rather than only
   on a bounded recorded buffer.

6. **Two-stage CFO acquisition (±50 Hz), ordered before fine timing.** A real SSB rig's dial and
   local oscillator are never exactly on frequency (~1 ppm at 14 MHz ≈ 14 Hz, plus operator tuning
   error). A coarse Moose estimate at a short lag (`fft_size/2`) plus a fine estimate at the full
   symbol lag are combined via their ambiguity periods (`Δ = fs/symbol_len`) to resolve offsets
   the fine-only estimator alone would wrap on. The CFO estimate is computed from the sync
   detector's own ring (no extra buffering) and applied to derotate the confirmation window
   *before* the confirm/refine cross-correlation, and again as a single whole-frame correction
   before demodulation — extending tolerance from ~15 Hz (fine-only) to ±50 Hz, verified to
   survive combined with Watterson fading (the two Schmidl-Cox preamble halves fade together, so
   their phase difference — the offset estimate — is preserved under fading too).

## Consequences

- **Wire-format break.** The Phase 1 waveform (carrier offset, preamble content, TX conditioning
  chain, header symbol count) is not compatible with the pre-Phase-1 waveform. Both ends of a
  link must run Phase-1-or-later code; there is no backward compatibility and none was attempted
  (Coppa is pre-1.0 with no deployed installed base to protect).
- **A real, quantified, accepted residual gap.** Levels 1–4 (BPSK/QPSK, the lowest-order, most
  redundant modes) still clear roughly 3 dB later than the pre-Phase-1 baseline after the TX
  conditioning fix, root-caused only as far as "not the preamble-domination effect" — deferred to
  Phase 2's dB-harvest work rather than blocking this phase, since Phase 1's purpose is occupying
  a realistic band and preamble, not final dB optimization. See BENCHMARKS.md for the full
  before/after tables.
- **A known, deliberately-not-recovered detection-margin trade-off.** `SyncDetector` runs on raw
  (unfiltered) samples rather than pre-filtered ones, to keep the O(1) CPU budget — a
  continuous 601-tap RX bandpass ahead of detection cost ~13x everything else combined and blew
  the 0.005x budget 4x over. This costs ~9 dB of detection margin on a genuinely noisy channel
  (does not affect false-positive rejection of noise/tones, which the confirm step already
  handles without pre-filtering). Recovering that margin (a cheaper/shorter filter, or
  downsampling before detection) is a reasonable future improvement, out of this phase's scope.
- **CFO tolerance is now ±50 Hz, not unlimited.** Beyond that the two-stage ambiguity resolution
  itself wraps; sample-clock offset remains uncorrected. This comfortably covers realistic HF
  oscillator + tuning error but is still a bounded envelope, not a general PLL.
- **The bench harness gained first-class `--ssb` and `--cfo` flags**, making the SSB-reality and
  CFO-tolerance claims in this ADR independently reproducible rather than asserted.
- **A real, newly-discovered Watterson-fading regression on sparse-pilot HF profiles**,
  found by this phase's own gate (Task 8), not by any individual task's review. AWGN improved
  for `hf_standard`-profile levels (1–4, see above), but the same levels' *fading* performance
  regressed sharply — levels 1–3 cleared FER≤10% on Watterson Moderate at 21 dB pre-phase and
  never clear it post-phase. Root-caused to the protected header (Golay+CRC+2D estimation,
  unchanged code) failing its CRC far more often post-phase specifically on sparse-pilot
  profiles (`hf_standard`/`hf_wide`/`hf_narrow`); the dense-pilot `hf_robust` profile is
  unaffected. Every task in this phase touched the header's channel-estimation inputs
  (carrier layout, preamble, RX bandpass, sync/CFO) without any task running a full Watterson
  sweep against the actual default per-level profiles — this ADR's decisions are the likely
  cause but the exact commit was not bisected (out of the phase-gate's scope). See
  BENCHMARKS.md's "Phase 1 (radio reality): final acceptance sweep" section for full data;
  flagged as a priority follow-up, not silently deferred.

## Related

- `docs/analysis/2026-07-03-world-class-gap-analysis.md` — the gap analysis that motivated this
  phase.
- `docs/superpowers/plans/2026-07-03-phase1-radio-reality.md` — the phase's task-by-task plan.
- `BENCHMARKS.md`, "2026-07 — Radio-reality Phase 1" and "Phase 1 (radio reality) — final
  acceptance sweep" sections — the measured regression, recovery, and final acceptance numbers.
