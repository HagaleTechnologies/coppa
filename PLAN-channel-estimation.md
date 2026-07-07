# PLAN: OFDM Channel Estimation & Noise Variance Correctness

**Status:** Spec — ready for implementation
**Date:** 2026-07-06
**Investigated by:** Claude (Fable 5), all findings verified by reading TX and RX paths end-to-end

---

## Verdict on the suspected pilot-reference bug: NOT A BUG

A prior review flagged `extract_pilot_info` (`crates/coppa-codec/src/ofdm/coppa_modem.rs:557-567`)
for hardcoding `Complex32::new(1.0, 0.0)` as the known pilot reference, suspecting the
transmitter might send ±1 alternating or PN-sequence pilots, which would invert channel
estimates.

**Verified false.** `CoppaPilotPattern::insert_pilots` (`pilots.rs:185-206`) writes
`Complex32::new(1.0, 0.0)` at every pilot position, for both even and odd symbols. The
legacy `PilotPattern` (`pilots.rs:17-77`) likewise fills `pilot_values` with `+1.0` in
all branches. TX and RX agree; the tests at `pilots.rs:258-269` and `312-318` pin this.
TX/RX symbol numbering also agrees: both sides use `sym_idx` for header symbols and
`global_sym = num_header_syms + sym_idx` for payload (`coppa_modem.rs:207/238` vs
`337/367`), and RX's hardcoded 48-bit header length matches `CoppaHeader::to_bits()`
(6 bytes, `frame.rs:244-...`). No fix needed for the reference itself.

*(Optional, out of scope: all-+1.0 pilots put a deterministic spectral line on the pilot
carriers and correlate with narrowband interferers. A length-matched PN pilot sequence,
known to both sides, is standard practice. Not a correctness issue — note only.)*

However, the investigation of this path found **three real defects** in how those pilots
are *used*. They share one root cause and one fix.

---

## Defect 1 (critical): noise variance measures the channel, not the noise

**Location:** `crates/coppa-codec/src/ofdm/equalizer.rs:55-90`
(`LinearInterpolationEstimator::update`)

`update()` does, in this order:

1. Compute noise variance from pilot prediction residuals:
   `residual = |received − self.h_estimates[idx] * known|²` — using **`h_estimates` from
   *before* this update**.
2. Then overwrite `h_estimates` from this symbol's pilots.

Every call site in the payload soft paths creates a **fresh estimator per OFDM symbol**
(`coppa_modem.rs:456, 541`: `LinearInterpolationEstimator::new(total_active)` inside the
per-symbol loop; `new()` initializes `h_estimates` to all `1+0j`). So step 1 computes
`residual = |received_pilot − 1.0|²` — i.e. **the channel's deviation from unity gain,
not the noise**.

Concrete failure: channel `H = 0.5·e^{j45°}` (mild HF attenuation + phase), zero noise.
True σ² = 0. Measured: `|0.5e^{j45°} − 1|² ≈ 0.54`. The estimator reports σ² ≈ 0.54 —
i.e. it claims SNR ≈ −3 dB on a noiseless channel.

**Blast radius:**

- `mmse_equalize` (`equalizer.rs:138-158`) uses `noise_var` in the denominator
  `|H|² + σ²`. Grossly inflated σ² over-regularizes: equalized symbols are attenuated
  toward zero, hurting hard decisions.
- Far worse: `per_carrier_noise` (`equalizer.rs:34-51`) scales σ² by `1/|H[k]|²` and the
  result feeds the LDPC decoder's LLR computation in the transceiver. Overestimated
  noise → collapsed LLR magnitudes → belief propagation loses its ability to converge
  precisely in the near-threshold regime where LDPC coding gain matters most.
- **Why tests never caught it:** in loopback tests the channel IS unity (`H = 1`), so
  `|received − 1.0|²` happens to equal the true noise power. The bug only exists on
  channels with gain ≠ 1 or phase ≠ 0 — i.e. every real radio channel.

Note the subtlety for the fix: you cannot simply reorder (estimate H first, then compute
residuals), because the interpolating estimator passes **exactly** through each pilot —
same-symbol residuals would be identically zero. Correct noise estimation needs
information the current per-symbol design throws away. See "Fix design" below.

---

## Defect 2: the even/odd alternating pilot design is documented but not implemented

**Location:** `pilots.rs:116-126` (docs) vs `coppa_modem.rs:456-458, 541-543` (usage)

`CoppaPilotPattern`'s docstring: *"Even symbols place pilots at evenly-spaced positions;
odd symbols offset by half the spacing so that **together they provide denser channel
estimation coverage**."* The "together" never happens: each OFDM symbol constructs a
fresh estimator from only its own `num_pilots` pilots. The alternating offsets currently
provide zero benefit — arguably negative, since odd symbols have no pilots at the band
edges (odd indices start at `half_spacing`, `pilots.rs:144-146`), so edge carriers on odd
symbols are flat-extrapolated from an interior pilot (`equalizer.rs:107-119`).

---

## Defect 3: the fine-sync symbol is a full-band channel probe that the receiver discards

**Location:** TX `coppa_modem.rs:181-183, 263-265`; RX `coppa_modem.rs:317-318`

The transmitter sends a "fine sync symbol": known BPSK `+1.0` on **all** active carriers.
The receiver computes `data_start = timing_offset + 3 * symbol_len` and never
demodulates symbol index 2. (Fine *timing* uses LTS cross-correlation on the preamble in
`sync.rs:138-150` — check during implementation whether that correlator consumes this
symbol; channel estimation definitely does not.)

This symbol is an exact per-carrier channel measurement at frame start — the single most
valuable channel-estimation asset in the frame — and it is thrown away.

---

## Fix design: one persistent estimator per frame, seeded by the fine-sync symbol

All three defects fall to the same change. In each demodulation path (`demodulate`,
`demodulate_soft`, `demodulate_soft_coded`):

1. **Hoist the estimator out of the per-symbol loop.** Create one
   `LinearInterpolationEstimator` per frame, before the header loop. Replace
   `equalize_carriers`'s internal construction (`coppa_modem.rs:570-579`) with a version
   that takes `&mut estimator`.

2. **Seed it from the fine-sync symbol.** Demodulate symbol index 2
   (`timing_offset + 2 * symbol_len`), treat every active carrier as a pilot with known
   value `+1.0`, and call `update()` once. This yields an exact full-band `H` estimate
   with no interpolation. (The `update()` API already accepts arbitrary
   `(idx, received, known)` tuples — no signature change needed.)

3. **Fix `update()`'s noise estimation to be well-defined under persistence.** With a
   persistent estimator, the step-1 residual `|received − H_prev[idx]·known|²` becomes
   meaningful: `H_prev` is last symbol's (or the seed's) estimate, so the residual is
   noise **plus channel drift over one symbol** — an acceptable, slightly conservative
   noise proxy when coherence time ≫ symbol duration (true for HF doppler spreads
   ≲ 1 Hz vs. tens-of-ms symbols; document this assumption in the code).
   Two required changes inside `update()`:
   - Smooth the noise estimate across symbols instead of overwriting:
     `self.noise_var = 0.9 * self.noise_var + 0.1 * measured` (EMA; keep the
     `.max(1e-10)` floor). A single-symbol fade shouldn't whipsaw the LLR scale.
   - Smooth the channel update as well: `H_new[k] = (1−α)·H_old[k] + α·(Y/X)[k]` with
     `α ≈ 0.5` at pilot positions (tunable constant, document it). This both tracks the
     channel and preserves the seed's full-band information at carriers the current
     symbol's sparse pilots don't cover. Interpolation between pilots then blends
     old-and-new naturally. **This is what finally realizes the even/odd "denser
     coverage" design:** consecutive symbols update complementary carrier positions of
     the same persistent `H`.

4. **First-update special case.** Keep `new()`'s all-ones init, but skip the noise
   measurement on the very first `update()` call of a frame (add a `seeded: bool` or
   count updates): measuring against the all-ones prior is exactly Defect 1. When the
   fine-sync seed runs first this is belt-and-suspenders, but it also fixes the legacy
   `demodulate` path if someone calls it without seeding.

5. **Reset per frame.** The estimator must NOT persist across frames (different
   propagation instant, possibly different station). Frame-scoped construction gives
   this for free — just don't make it a field of `CoppaModem` (which is shared across
   frames and `&self`).

### What NOT to change

- `extract_pilot_info`'s `+1.0` known reference — correct as-is (see verdict above).
  Add a comment pointing at `insert_pilots` so the next reviewer doesn't re-flag it:
  the reference must stay in lockstep with the TX pilot values.
- `mmse_equalize` itself — the formula is fine once σ² is honest.
- The pilot patterns and TX side — no protocol change; this is receiver-only, fully
  backward/forward compatible on air.

---

## Test plan

The existing suite only exercises `H = 1` loopback, which is exactly the blind spot.
Add a tiny channel simulator to the codec test utils (no new crate):

```rust
/// Apply gain*e^{jφ} per-carrier channel + AWGN at a given SNR to modulated samples.
/// Implement as a short FIR (2-3 taps) for frequency selectivity + Gaussian noise.
fn apply_channel(samples: &[f32], taps: &[Complex32], snr_db: f32) -> Vec<f32>
```

1. **Noise-variance honesty test (regression for Defect 1):** modulate a frame, pass
   through `taps = [0.5·e^{j45°}]` (flat non-unity channel) + AWGN at 20 dB SNR.
   Assert the estimator's `noise_variance()` after the payload is within 3 dB of the
   injected noise power — the current code fails this by ~14 dB.
2. **Attenuated-channel round-trip:** frame through flat 0.5-gain channel, no noise →
   `demodulate` recovers the exact payload (currently at risk from MMSE
   over-regularization).
3. **Frequency-selective round-trip:** 2-tap multipath + 15 dB SNR → `demodulate_soft_coded`
   + LDPC decode succeeds across all speed levels 1–9 (parameterized test). This also
   covers the hardcoded-1944 concern flagged in PLAN-hardening.md by exercising every
   rate through the same pipeline.
4. **Fine-sync seeding test:** assert the estimator's `H` after seeding matches the
   injected channel per-carrier within tolerance (expose `h_estimates` behind
   `#[cfg(test)]` or a getter).
5. **Near-threshold soft-decode comparison (the payoff):** at SNR just above the LDPC
   threshold with multipath, decode success rate over N=100 random frames must be
   ≥ the pre-change baseline. Record the baseline before making changes.

## Implementation order

1. Channel simulator + failing tests 1–2 (they fail against current code — proves the bug).
2. `update()` changes (EMA noise, first-update skip, α-blend) — `equalizer.rs` only.
3. Persistent estimator + fine-sync seeding in the three demod paths — `coppa_modem.rs`.
4. Tests 3–5 green; run `cargo test --workspace`.
5. Update `PLAN-hardening.md` §4 (noise-floor testing) to reference the new simulator.
