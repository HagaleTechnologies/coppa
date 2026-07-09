//! OFDM synchronization: preamble generation and CFO estimation.
//!
//! The live timing-detection path (Schmidl-Cox autocorrelation + confirm +
//! first-path refinement) now lives in [`super::sync_detector::SyncDetector`],
//! which replaced this module's old batch/O(N) `SchmidlCox`, `LtsCorrelator`,
//! `detect_coppa_version`, `coppa_version_correlation`, and `detect_coppa_sync`
//! (all deleted; their behaviors are covered by `SyncDetector`'s own tests).
//! This module retains the pieces that stay: preamble generation (used both to
//! transmit and as `SyncDetector`'s cached confirmation reference) and CFO
//! estimation/removal (still applied post-sync in `coppa_modem.rs`; two-stage
//! CFO estimation folding into `SyncDetector` itself is Task 6).
use coppa_dsp::fft::FftProcessor;
use num_complex::Complex32;

use super::CoppaProfile;

// ---------------------------------------------------------------------------
// Coppa version-keyed preamble sync
// ---------------------------------------------------------------------------

/// Generate a deterministic BPSK PN sequence for a given protocol version.
///
/// Uses a 7-bit LFSR with polynomial x^7 + x + 1 (taps at bits 6 and 0),
/// seeded with the version number (offset by 1 to avoid the all-zero state).
/// Returns `fft_size / 2` values, each +1.0 or -1.0.
pub fn coppa_pn_sequence(version: u8) -> Vec<f32> {
    let length = 480; // half of FFT size 960
    let mut state: u8 = version.wrapping_add(1); // avoid seed 0
    if state == 0 {
        state = 1;
    }
    // Keep only lower 7 bits
    state &= 0x7F;
    if state == 0 {
        state = 1;
    }

    let mut seq = Vec::with_capacity(length);
    for _ in 0..length {
        // Output bit is LSB
        let bit = state & 1;
        seq.push(if bit == 1 { 1.0f32 } else { -1.0f32 });
        // Feedback: new bit = bit6 XOR bit0
        let feedback = ((state >> 6) ^ (state & 1)) & 1;
        state = ((state >> 1) | (feedback << 6)) & 0x7F;
        if state == 0 {
            state = 1;
        }
    }
    seq
}

/// Even FFT bins inside the profile's active band — the preamble comb.
/// Even-only placement makes the symbol body periodic with N/2, which both
/// preserves the two-identical-halves Schmidl-Cox structure and enables the
/// lag-N/2 coarse CFO estimate (±fs/(2·(N/2)) = ±50 Hz at N=960/48k).
pub fn preamble_comb_bins(profile: &CoppaProfile) -> Vec<usize> {
    let first = profile.first_active_bin();
    let last = profile.carrier_offset + profile.total_active_carriers();
    (first..=last).filter(|b| b % 2 == 0).collect()
}

/// Newman phases for K tones: phi_k = pi*(k-1)^2/K — near-minimal PAPR (~3 dB)
/// for an equal-amplitude comb. `rotation` cyclically shifts the phase sequence
/// to key the preamble by protocol version without changing its envelope class.
fn newman_phases(k_tones: usize, rotation: usize) -> Vec<f32> {
    (0..k_tones)
        .map(|k| {
            let kk = (k + rotation) % k_tones;
            std::f32::consts::PI * (kk * kk) as f32 / k_tones as f32
        })
        .collect()
}

/// Generate a 2-symbol Schmidl-Cox preamble for a given Coppa profile and version.
///
/// Places a Newman-phase comb (near-minimal PAPR, ~3 dB) on the profile's in-band
/// even FFT bins ([`preamble_comb_bins`]), Hermitian-mirrored so the IFFT output is
/// real-valued. `version` cyclically rotates the Newman phase assignment, keying the
/// preamble per protocol version while preserving its low-PAPR envelope class.
/// Returns 2 identical unit-RMS symbols, each with its cyclic prefix prepended; the
/// two-identical-halves Schmidl-Cox structure and per-symbol period N/2 are both
/// preserved (see docs/superpowers/plans/2026-07-03-phase1-radio-reality.md Task 2).
pub fn generate_coppa_preamble(profile: &CoppaProfile, version: u8) -> Vec<f32> {
    let n = profile.fft_size;
    let cp = profile.cp_samples;
    let fft = FftProcessor::new(n);
    let bins = preamble_comb_bins(profile);
    let phases = newman_phases(bins.len(), version as usize);

    let mut freq = vec![Complex32::new(0.0, 0.0); n];
    for (&bin, &ph) in bins.iter().zip(phases.iter()) {
        let v = Complex32::new(ph.cos(), ph.sin());
        freq[bin] = v;
        freq[n - bin] = v.conj();
    }
    let time = fft.inverse(&freq);

    let mut symbol = Vec::with_capacity(cp + n);
    symbol.extend(time[n - cp..].iter().map(|s| s.re));
    symbol.extend(time.iter().map(|s| s.re));

    // Unit-RMS normalize: section power leveling happens in the modem's TX power
    // plan, which needs a known reference.
    let rms = (symbol.iter().map(|x| x * x).sum::<f32>() / symbol.len() as f32).sqrt();
    if rms > 1e-12 {
        for s in &mut symbol {
            *s /= rms;
        }
    }

    let mut out = Vec::with_capacity(2 * symbol.len());
    out.extend_from_slice(&symbol);
    out.extend_from_slice(&symbol);
    out
}

// ---------------------------------------------------------------------------
// Carrier frequency offset (CFO) estimation + correction
// ---------------------------------------------------------------------------

/// Analytic (complex) signal via FFT-Hilbert. Mirrors the watterson/frequency_shift routine:
/// keep DC, double positive-frequency bins, zero the negative ones, then inverse FFT.
///
/// `pub(crate)`: also used by `sync_detector` to derotate its real-valued confirmation
/// window by the two-stage CFO estimate before the confirm/first-path xcorr steps.
pub(crate) fn analytic_signal(x: &[f32]) -> Vec<Complex32> {
    let n = x.len();
    if n == 0 {
        return Vec::new();
    }
    let fft = FftProcessor::new(n);
    let xc: Vec<Complex32> = x.iter().map(|&v| Complex32::new(v, 0.0)).collect();
    let mut xf = fft.forward(&xc);
    let half = n / 2;
    for s in xf.iter_mut().take(n).skip(half + 1) {
        *s = Complex32::new(0.0, 0.0);
    }
    for s in xf.iter_mut().take(half).skip(1) {
        *s *= 2.0;
    }
    // Odd n has no exact Nyquist bin, so `half` is a positive frequency too.
    if n % 2 == 1 && half >= 1 {
        xf[half] *= 2.0;
    }
    fft.inverse(&xf)
}

/// Single-lag Moose CFO estimate: `arg(Sum_m z*[start+m]*z[start+m+lag]) * fs / (2*pi*lag)`,
/// summed over `m in 0..len`. This is the one Moose correlation shared by both the
/// legacy single-lag `estimate_cfo_hz` (lag = symbol_len, unambiguous within
/// ±fs/(2*symbol_len)) and the two-stage `estimate_cfo_two_stage` below (lag = N/2 for a
/// wide-but-noisy coarse estimate, disambiguating a lag = symbol_len fine estimate).
/// Returns 0.0 if the window doesn't fit (rather than panicking) so callers can pass a
/// possibly-short trailing window without their own bounds bookkeeping.
fn moose_lag_estimate(
    analytic: &[Complex32],
    start: usize,
    lag: usize,
    len: usize,
    sample_rate: f32,
) -> f32 {
    use std::f32::consts::TAU;
    if lag == 0 || len == 0 || start + len + lag > analytic.len() {
        return 0.0;
    }
    let mut p = Complex32::new(0.0, 0.0);
    for m in 0..len {
        p += analytic[start + m].conj() * analytic[start + m + lag];
    }
    p.arg() * sample_rate / (TAU * lag as f32)
}

/// Estimate carrier frequency offset (Hz) from the Schmidl-Cox preamble (two identical
/// `symbol_len` blocks at `timing_offset`). Unambiguous within ±sample_rate/(2*symbol_len).
pub fn estimate_cfo_hz(
    samples: &[f32],
    timing_offset: usize,
    symbol_len: usize,
    sample_rate: f32,
) -> f32 {
    let end = (timing_offset + 2 * symbol_len).min(samples.len());
    if symbol_len == 0 || end <= timing_offset + symbol_len {
        return 0.0;
    }
    let a = analytic_signal(&samples[timing_offset..end]);
    let len = symbol_len.min(a.len().saturating_sub(symbol_len));
    moose_lag_estimate(&a, 0, symbol_len, len, sample_rate)
}

/// Two-stage Moose CFO from the 2-symbol preamble at `coarse_start` (see the task/design
/// doc for the derivation): a coarse estimate at lag `fft_size/2` (unambiguous within
/// ±fs/(fft_size), e.g. ±50 Hz at fft_size=960/fs=48k) disambiguates a fine estimate at lag
/// `symbol_len = fft_size+cp` (unambiguous within ±fs/(2*symbol_len) ~ ±19 Hz at this
/// profile, but far more precise since it integrates over a much longer lag/window).
///
/// `analytic` must be an analytic-signal window covering at least
/// `coarse_start..coarse_start+2*symbol_len` (the full 2-symbol preamble); `coarse_start`
/// is the sample index, within `analytic`, of the very first preamble sample (i.e. the
/// start of its own cyclic prefix).
///
/// Math (locked, see task brief): `f_coarse` sums lag-`fft_size/2` products over
/// `m in cp..cp+(fft_size-fft_size/2)` (one half of the first preamble symbol's body,
/// correlated against the other half — the comb's even-bin placement makes the body
/// periodic with `fft_size/2`, which is exactly what a `fft_size/2`-sample lag measures).
/// `f_fine` sums lag-`symbol_len` products over the whole 2-symbol preamble
/// (`m in 0..symbol_len`) — the ordinary single-lag Moose estimate `estimate_cfo_hz` also
/// computes, just operating directly on an already-analytic window instead of re-deriving
/// one from real samples. `k = round((f_coarse-f_fine)/Delta)` picks the fine estimate's
/// wrap count using the coarse (wide-range, low-precision) estimate as a coarse compass;
/// `f_hat = f_fine + k*Delta` is the final, wide-range AND precise estimate.
pub fn estimate_cfo_two_stage(
    analytic: &[Complex32],
    coarse_start: usize,
    fft_size: usize,
    cp: usize,
    sample_rate: f32,
) -> f32 {
    let n_half = fft_size / 2;
    let symbol_len = fft_size + cp;
    let coarse_len = fft_size.saturating_sub(n_half);
    let body_start = coarse_start + cp;

    let f_coarse = moose_lag_estimate(analytic, body_start, n_half, coarse_len, sample_rate);
    let f_fine = moose_lag_estimate(analytic, coarse_start, symbol_len, symbol_len, sample_rate);

    let delta = sample_rate / symbol_len as f32;
    if delta <= 0.0 {
        return f_fine;
    }
    let k = ((f_coarse - f_fine) / delta).round();
    f_fine + k * delta
}

/// Remove a carrier frequency offset of `cfo_hz` from a real passband signal (de-rotate the
/// analytic signal by -cfo and take the real part).
pub fn remove_cfo(samples: &[f32], cfo_hz: f32, sample_rate: f32) -> Vec<f32> {
    use std::f32::consts::TAU;
    let a = analytic_signal(samples);
    a.iter()
        .enumerate()
        .map(|(i, &z)| {
            let ph = -TAU * cfo_hz * i as f32 / sample_rate;
            (z * Complex32::new(ph.cos(), ph.sin())).re
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_cfo_recovers_injected_offset() {
        use crate::ofdm::coppa_modem::CoppaModem;
        use crate::ofdm::frame::{CoppaFrameType, CoppaHeader};
        let profile = CoppaProfile::hf_standard();
        let symbol_len = profile.fft_size + profile.cp_samples;
        let modem = CoppaModem::new(profile.clone(), 1);
        // Build a real frame (preamble starts at sample 0).
        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: 4,
            codewords: 1,
        };
        let symbols = vec![num_complex::Complex32::new(1.0, 0.0); 200];
        let frame = modem.modulate_mapped(&header, &symbols, 6.0);
        // Inject +5 Hz by removing -5 Hz.
        let injected = remove_cfo(&frame, -5.0, profile.sample_rate as f32);
        let est = estimate_cfo_hz(&injected, 0, symbol_len, profile.sample_rate as f32);
        assert!((est - 5.0).abs() < 1.0, "estimate {est} should be ~+5 Hz");
        // And removing it returns ~0 estimate.
        let corrected = remove_cfo(&injected, est, profile.sample_rate as f32);
        let est2 = estimate_cfo_hz(&corrected, 0, symbol_len, profile.sample_rate as f32);
        assert!(est2.abs() < 1.0, "residual {est2} should be ~0");
    }

    #[test]
    fn two_stage_cfo_recovers_large_offsets() {
        // +-47 Hz is beyond the old +-19 Hz single-lag-1260 wrap; must recover to <1 Hz
        // error using the coarse (lag 480) estimate to disambiguate the fine one.
        use crate::ofdm::coppa_modem::CoppaModem;
        use crate::ofdm::frame::{CoppaFrameType, CoppaHeader};
        let profile = CoppaProfile::hf_standard();
        let modem = CoppaModem::new(profile.clone(), 1);
        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: 4,
            codewords: 1,
        };
        let symbols = vec![Complex32::new(1.0, 0.0); 200];
        let frame = modem.modulate_mapped(&header, &symbols, 6.0);

        for inj in [-47.0f32, -25.0, 13.0, 31.0, 47.0] {
            // Inject `inj` Hz by removing `-inj` Hz (frame's preamble starts at sample 0).
            let shifted = remove_cfo(&frame, -inj, profile.sample_rate as f32);
            let analytic = analytic_signal(&shifted);
            let est = estimate_cfo_two_stage(
                &analytic,
                0,
                profile.fft_size,
                profile.cp_samples,
                profile.sample_rate as f32,
            );
            assert!(
                (est - inj).abs() < 1.0,
                "injected {inj} Hz: estimate {est} should be within 1 Hz"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Coppa version-keyed preamble tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_coppa_pn_sequences_are_different_per_version() {
        let pn1 = coppa_pn_sequence(1);
        let pn2 = coppa_pn_sequence(2);
        assert_eq!(pn1.len(), 480);
        assert_eq!(pn2.len(), 480);
        assert_ne!(pn1, pn2, "PN sequences for different versions must differ");
    }

    #[test]
    fn test_coppa_pn_sequences_are_bpsk() {
        for version in 1..=4u8 {
            let pn = coppa_pn_sequence(version);
            assert_eq!(pn.len(), 480);
            for (i, &val) in pn.iter().enumerate() {
                assert!(
                    val == 1.0 || val == -1.0,
                    "Version {} index {}: expected +1.0 or -1.0, got {}",
                    version,
                    i,
                    val
                );
            }
        }
    }

    // `test_coppa_version_detect_v1`/`test_coppa_version_detect_v1_not_v2` (which
    // exercised the legacy `detect_coppa_version`/`coppa_version_correlation`
    // full-search correlator, deleted in Task 5's `SyncDetector` migration) are
    // gone along with those functions. The meaningful decorrelation guarantee
    // (compared at the true aligned samples, not a spurious best-lag search) is
    // covered by `preamble_versions_are_distinguishable` below; version-keyed
    // detection itself is covered by `SyncDetector`'s own tests in
    // `sync_detector.rs`.

    // -----------------------------------------------------------------------
    // Newman-phase in-band preamble tests (Phase 1 Task 2; see
    // docs/superpowers/plans/2026-07-03-phase1-radio-reality.md)
    // -----------------------------------------------------------------------

    #[test]
    fn preamble_is_in_band_and_low_papr() {
        let p = CoppaProfile::hf_standard();
        let pre = generate_coppa_preamble(&p, 1);
        // (a) Spectral confinement: FFT one symbol body; all energy on even bins 8..=54.
        let n = p.fft_size;
        let cp = p.cp_samples;
        let body: Vec<num_complex::Complex32> = pre[cp..cp + n]
            .iter()
            .map(|&x| num_complex::Complex32::new(x, 0.0))
            .collect();
        let fft = coppa_dsp::fft::FftProcessor::new(n);
        let spec = fft.forward(&body);
        let total: f32 = spec[1..n / 2].iter().map(|c| c.norm_sqr()).sum();
        let in_band: f32 = (8..=54).step_by(2).map(|b| spec[b].norm_sqr()).sum();
        assert!(
            in_band / total > 0.999,
            "preamble energy must sit on the in-band even comb"
        );
        // (b) PAPR: Newman phasing keeps the comb's PAPR far below the ~11 dB PN-BPSK
        // comb it replaces (and the ~19.8 dB all-ones probe symbol it also replaces).
        // NOTE ON THRESHOLD: the plan's design-decision doc (Task 2 rationale) cites the
        // Newman ~3 dB figure, which is the *asymptotic* result for large tone counts;
        // it does not hold tightly at this profile's K=24 in-band tones. Independently
        // verified (both this crate's FFT and a from-scratch reference DFT in Python)
        // that version 1's rotation measures ~4.93 dB, and versions 1-4 range ~4.9-5.8
        // dB. 6.0 dB keeps meaningful margin above that measured range while still
        // asserting the real, large win over the schemes being replaced.
        let rms = (pre.iter().map(|x| x * x).sum::<f32>() / pre.len() as f32).sqrt();
        let peak = pre.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        let papr_db = 20.0 * (peak / rms).log10();
        assert!(
            papr_db < 6.0,
            "Newman preamble PAPR should be far below the ~11 dB PN-BPSK comb it \
             replaces, got {papr_db}"
        );
        // (c) Two identical halves at symbol_len lag (Schmidl-Cox structure preserved).
        let sym = n + cp;
        for i in 0..sym {
            assert!((pre[i] - pre[i + sym]).abs() < 1e-6);
        }
    }

    #[test]
    fn preamble_versions_are_distinguishable() {
        let p = CoppaProfile::hf_standard();
        let a = generate_coppa_preamble(&p, 1);
        let b = generate_coppa_preamble(&p, 2);
        let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        let ea: f32 = a.iter().map(|x| x * x).sum();
        assert!(
            dot.abs() / ea < 0.5,
            "version-rotated Newman combs must decorrelate"
        );
    }
}
