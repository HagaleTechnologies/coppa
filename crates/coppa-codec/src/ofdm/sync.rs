//! OFDM synchronization: Schmidl-Cox timing and CFO estimation.
use coppa_dsp::fft::FftProcessor;
use num_complex::Complex32;

use super::CoppaProfile;

/// Schmidl-Cox synchronization detector.
///
/// Uses auto-correlation of the Short Training Sequence (STS) to detect
/// coarse timing and estimate carrier frequency offset.
///
/// For proper CFO estimation, the input should be complex baseband (I/Q).
/// When used with real-valued passband signals, CFO estimation is limited
/// to detecting polarity inversions only — the imaginary correlation
/// component is always zero for real signals.
pub struct SchmidlCox {
    fft_size: usize,
    cp_length: usize,
    /// Detection threshold for the normalized correlation metric.
    threshold: f32,
}

impl SchmidlCox {
    pub fn new(fft_size: usize, cp_length: usize) -> Self {
        Self {
            fft_size,
            cp_length,
            threshold: 0.7,
        }
    }

    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold;
        self
    }

    /// Detect the start of an OFDM frame using Schmidl-Cox auto-correlation
    /// on real-valued samples.
    ///
    /// The STS consists of two identical halves. The auto-correlation between
    /// the two halves produces a plateau at the correct timing position.
    ///
    /// Returns `Some((timing_offset, cfo_estimate_hz))` if a frame is detected.
    /// Note: CFO estimation requires complex baseband; use `detect_complex` for
    /// accurate CFO.
    pub fn detect(&self, samples: &[f32], _sample_rate: f32) -> Option<(usize, f32)> {
        let half = self.fft_size / 2;
        if samples.len() < self.fft_size + self.cp_length {
            return None;
        }

        let mut best_metric = 0.0f32;
        let mut best_pos = 0usize;

        let search_end = samples.len().saturating_sub(self.fft_size + self.cp_length);

        for d in 0..search_end {
            let mut p_re = 0.0f32;
            let mut r = 0.0f32;
            let mut r1 = 0.0f32;

            for m in 0..half {
                let s1 = samples[d + m];
                let s2 = samples[d + m + half];
                p_re += s1 * s2;
                r += s2 * s2;
                r1 += s1 * s1;
            }

            // Standard Schmidl-Cox metric: |P(d)|^2 / R(d)^2
            // Use geometric mean of both halves' energy for better normalization
            let denom = (r1 * r).max(1e-20);
            let metric = (p_re * p_re) / denom;

            if metric > best_metric {
                best_metric = metric;
                best_pos = d;
            }
        }

        if best_metric >= self.threshold {
            // Real-valued signals cannot estimate CFO via phase rotation
            Some((best_pos, 0.0))
        } else {
            None
        }
    }

    /// Detect OFDM frame using complex baseband samples for proper CFO estimation.
    ///
    /// The complex auto-correlation `P(d) = Σ s*(d+m) · s(d+m+L)` produces a
    /// phase angle proportional to the carrier frequency offset.
    pub fn detect_complex(&self, samples: &[Complex32], sample_rate: f32) -> Option<(usize, f32)> {
        let half = self.fft_size / 2;
        if samples.len() < self.fft_size + self.cp_length {
            return None;
        }

        let mut best_metric = 0.0f32;
        let mut best_pos = 0usize;
        let mut best_angle = 0.0f32;

        let search_end = samples.len().saturating_sub(self.fft_size + self.cp_length);

        for d in 0..search_end {
            let mut p = Complex32::new(0.0, 0.0);
            let mut r = 0.0f32;
            let mut r1 = 0.0f32;

            for m in 0..half {
                let s1 = samples[d + m];
                let s2 = samples[d + m + half];
                // P(d) = Σ conj(s1) * s2
                p += s1.conj() * s2;
                r += s2.norm_sqr();
                r1 += s1.norm_sqr();
            }

            let denom = (r1 * r).max(1e-20);
            let metric = p.norm_sqr() / denom;

            if metric > best_metric {
                best_metric = metric;
                best_pos = d;
                best_angle = p.im.atan2(p.re);
            }
        }

        if best_metric >= self.threshold {
            let cfo_hz = -best_angle * sample_rate / (std::f32::consts::PI * self.fft_size as f32);
            Some((best_pos, cfo_hz))
        } else {
            None
        }
    }
}

/// Cross-correlation based fine timing using the Long Training Sequence.
pub struct LtsCorrelator {
    /// Known LTS frequency-domain values for all active subcarriers.
    lts_freq: Vec<Complex32>,
    fft_size: usize,
}

impl LtsCorrelator {
    pub fn new(lts_freq: Vec<Complex32>, fft_size: usize) -> Self {
        Self { lts_freq, fft_size }
    }

    /// Find fine timing offset by cross-correlating with known LTS.
    /// Returns the sample offset of the LTS start within `samples`.
    pub fn find_lts(&self, samples: &[f32]) -> Option<usize> {
        if samples.len() < self.fft_size {
            return None;
        }

        use coppa_dsp::fft::FftProcessor;
        let fft = FftProcessor::new(self.fft_size);

        // Build frequency-domain reference with Hermitian symmetry so the
        // IFFT produces a real-valued time-domain sequence.
        let n = self.fft_size;
        let mut freq = vec![Complex32::new(0.0, 0.0); n];
        for (i, &v) in self.lts_freq.iter().enumerate() {
            let bin = i + 1; // skip DC
            if bin < n / 2 {
                freq[bin] = v;
                freq[n - bin] = v.conj();
            } else if bin == n / 2 {
                // Nyquist bin: must be real
                freq[bin] = Complex32::new(v.re, 0.0);
            }
        }
        let reference = fft.inverse(&freq);

        let mut best_corr = 0.0f32;
        let mut best_pos = 0usize;
        // Compute reference energy for threshold
        let ref_energy: f32 = reference.iter().map(|c| c.re * c.re).sum();

        let search_end = samples.len().saturating_sub(self.fft_size);
        for d in 0..search_end {
            let mut corr = 0.0f32;
            for n in 0..self.fft_size {
                corr += samples[d + n] * reference[n].re;
            }
            let corr_abs = corr.abs();
            if corr_abs > best_corr {
                best_corr = corr_abs;
                best_pos = d;
            }
        }

        // Only return if correlation exceeds a meaningful threshold
        let threshold = ref_energy.sqrt() * 0.3;
        if best_corr > threshold {
            Some(best_pos)
        } else {
            None
        }
    }
}

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

/// Generate a 2-symbol Schmidl-Cox preamble for a given Coppa profile and version.
///
/// Each symbol has the PN sequence placed on even-numbered FFT bins with
/// Hermitian symmetry enforced so the IFFT output is real-valued.
/// Returns 2 symbols, each with its cyclic prefix prepended.
pub fn generate_coppa_preamble(profile: &CoppaProfile, version: u8) -> Vec<f32> {
    let n = profile.fft_size;
    let cp = profile.cp_samples;
    let fft = FftProcessor::new(n);
    let pn = coppa_pn_sequence(version);

    // Build frequency-domain symbol: PN values on even bins (2, 4, 6, ...)
    let mut freq = vec![Complex32::new(0.0, 0.0); n];
    for (i, &val) in pn.iter().enumerate() {
        let bin = (i + 1) * 2; // even bins: 2, 4, 6, ...
        if bin < n / 2 {
            freq[bin] = Complex32::new(val, 0.0);
            freq[n - bin] = Complex32::new(val, 0.0); // conj of real is itself
        } else if bin == n / 2 {
            freq[bin] = Complex32::new(val, 0.0);
        }
    }

    let time = fft.inverse(&freq);

    // Build one symbol: CP + IFFT output (real parts)
    let cp_start = n - cp;
    let symbol_len = cp + n;
    let mut symbol = Vec::with_capacity(symbol_len);
    symbol.extend(time[cp_start..].iter().map(|s| s.re));
    symbol.extend(time.iter().map(|s| s.re));

    // 2-symbol preamble
    let mut output = Vec::with_capacity(symbol_len * 2);
    output.extend_from_slice(&symbol);
    output.extend_from_slice(&symbol);
    output
}

/// Compute normalized cross-correlation of samples against a version's preamble.
///
/// Returns a value in 0.0..1.0 representing the absolute correlation strength.
pub fn coppa_version_correlation(samples: &[f32], profile: &CoppaProfile, version: u8) -> f32 {
    let reference = generate_coppa_preamble(profile, version);
    let ref_len = reference.len();
    if samples.len() < ref_len {
        return 0.0;
    }

    let ref_energy: f32 = reference.iter().map(|x| x * x).sum();
    if ref_energy < 1e-20 {
        return 0.0;
    }

    let mut best = 0.0f32;
    let search_end = samples.len() - ref_len + 1;
    for d in 0..search_end {
        let mut corr = 0.0f32;
        let mut sig_energy = 0.0f32;
        for (i, &r) in reference.iter().enumerate() {
            let s = samples[d + i];
            corr += s * r;
            sig_energy += s * s;
        }
        let denom = (ref_energy * sig_energy).sqrt().max(1e-20);
        let normalized = corr.abs() / denom;
        if normalized > best {
            best = normalized;
        }
    }
    best
}

/// Try all defined protocol versions (1-4) and return the best match.
///
/// Returns `Some((version, timing_offset))` if the best correlation exceeds
/// the detection threshold (0.5), or `None` if no version matches.
pub fn detect_coppa_version(samples: &[f32], profile: &CoppaProfile) -> Option<(u8, usize)> {
    let threshold = 0.5f32;
    let mut best_version = 0u8;
    let mut best_corr = 0.0f32;
    let mut best_offset = 0usize;

    for version in 1..=4u8 {
        let reference = generate_coppa_preamble(profile, version);
        let ref_len = reference.len();
        if samples.len() < ref_len {
            continue;
        }

        let ref_energy: f32 = reference.iter().map(|x| x * x).sum();
        if ref_energy < 1e-20 {
            continue;
        }

        let search_end = samples.len() - ref_len + 1;
        for d in 0..search_end {
            let mut corr = 0.0f32;
            let mut sig_energy = 0.0f32;
            for (i, &r) in reference.iter().enumerate() {
                let s = samples[d + i];
                corr += s * r;
                sig_energy += s * s;
            }
            let denom = (ref_energy * sig_energy).sqrt().max(1e-20);
            let normalized = corr.abs() / denom;
            if normalized > best_corr {
                best_corr = normalized;
                best_version = version;
                best_offset = d;
            }
        }
    }

    if best_corr >= threshold {
        Some((best_version, best_offset))
    } else {
        None
    }
}

/// Robust preamble timing detection via Schmidl-Cox autocorrelation.
///
/// The 2-symbol Coppa preamble is two identical OFDM symbols, so the received
/// signal satisfies `r[d+m] ≈ r[d+m+symbol_len]` across the preamble *even under
/// multipath* — the channel distorts both copies identically, so their
/// self-similarity survives fading that would break a cross-correlation against a
/// clean template. We detect that self-similarity instead.
///
/// Returns `Some((version, offset))` with `offset` = the start of the preamble
/// (same semantics as [`detect_coppa_version`]), or `None`. The version is nominal
/// (1); the live demod path uses only the timing offset.
pub fn detect_coppa_sync(samples: &[f32], profile: &CoppaProfile) -> Option<(u8, usize)> {
    let symbol_len = profile.fft_size + profile.cp_samples;
    if symbol_len == 0 || samples.len() < 2 * symbol_len {
        return None;
    }
    let search_end = samples.len() - 2 * symbol_len + 1;

    // Schmidl-Cox metric M(d) = |P(d)|^2 / (E1·E2), ≈ 1.0 when the two copies match.
    // We compute P(d) on the ANALYTIC (complex) signal: a carrier frequency offset rotates
    // the second copy relative to the first by a fixed phase, which leaves |P(d)|^2 invariant
    // (it only rotates P's angle). A real-valued autocorrelation instead picks up cos(rotation),
    // which collapses the metric past a few Hz of CFO and breaks detection — exactly the
    // failure this CFO path exists to fix. Working on the analytic signal makes coarse timing
    // CFO-tolerant; the de-rotation in the demod then removes the offset itself.
    let analytic = analytic_signal(samples);
    let threshold = 0.6f32;
    let mut best_metric = 0.0f32;
    let mut best_offset = 0usize;

    for d in 0..search_end {
        let mut p = Complex32::new(0.0, 0.0); // complex autocorrelation of the two copies
        let mut e1 = 0.0f32;
        let mut e2 = 0.0f32;
        for m in 0..symbol_len {
            let a = analytic[d + m];
            let b = analytic[d + m + symbol_len];
            p += a.conj() * b;
            e1 += a.norm_sqr();
            e2 += b.norm_sqr();
        }
        let denom = (e1 * e2).max(1e-20);
        let metric = p.norm_sqr() / denom;
        if metric > best_metric {
            best_metric = metric;
            best_offset = d;
        }
    }

    if best_metric < threshold {
        return None;
    }

    // Fine timing: the autocorrelation peak sits on a ~CP-wide plateau, which is too
    // coarse under fading (an off-by-tens-of-samples FFT window corrupts the header).
    // Refine within ±CP of the coarse offset by cross-correlating the clean reference
    // and taking the local argmax. A *local* search is robust even when the faded
    // preamble's absolute correlation is low — we only rank alignments near the
    // already-detected position, never threshold on the value.
    let reference = generate_coppa_preamble(profile, 1);
    let ref_len = reference.len();
    let lo = best_offset.saturating_sub(profile.cp_samples);
    let hi = (best_offset + profile.cp_samples).min(samples.len().saturating_sub(ref_len));
    if hi < lo {
        return Some((1, best_offset));
    }

    let mut best_xc = -1.0f32;
    let mut refined = best_offset;
    for d in lo..=hi {
        let mut corr = 0.0f32;
        let mut sig_e = 0.0f32;
        for (i, &r) in reference.iter().enumerate() {
            let s = samples[d + i];
            corr += s * r;
            sig_e += s * s;
        }
        let nc = corr.abs() / sig_e.sqrt().max(1e-20);
        if nc > best_xc {
            best_xc = nc;
            refined = d;
        }
    }
    Some((1, refined))
}

// ---------------------------------------------------------------------------
// Carrier frequency offset (CFO) estimation + correction
// ---------------------------------------------------------------------------

/// Analytic (complex) signal via FFT-Hilbert. Mirrors the watterson/frequency_shift routine:
/// keep DC, double positive-frequency bins, zero the negative ones, then inverse FFT.
fn analytic_signal(x: &[f32]) -> Vec<Complex32> {
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

/// Estimate carrier frequency offset (Hz) from the Schmidl-Cox preamble (two identical
/// `symbol_len` blocks at `timing_offset`). Unambiguous within ±sample_rate/(2*symbol_len).
pub fn estimate_cfo_hz(
    samples: &[f32],
    timing_offset: usize,
    symbol_len: usize,
    sample_rate: f32,
) -> f32 {
    use std::f32::consts::TAU;
    let end = (timing_offset + 2 * symbol_len).min(samples.len());
    if symbol_len == 0 || end <= timing_offset + symbol_len {
        return 0.0;
    }
    let a = analytic_signal(&samples[timing_offset..end]);
    let lim = symbol_len.min(a.len().saturating_sub(symbol_len));
    let mut p = Complex32::new(0.0, 0.0);
    for m in 0..lim {
        p += a[m].conj() * a[m + symbol_len];
    }
    p.arg() * sample_rate / (TAU * symbol_len as f32)
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
mod robust_sync_tests {
    use super::*;

    #[test]
    fn detect_coppa_sync_survives_multipath_echo() {
        let profile = CoppaProfile::hf_standard();
        let symbol_len = profile.fft_size + profile.cp_samples;
        let preamble = generate_coppa_preamble(&profile, 1);

        // Leading gap + preamble + tail (room for the demod's +3-symbol data_start).
        let lead = 137usize;
        let mut clean = vec![0.0f32; lead];
        clean.extend_from_slice(&preamble);
        clean.extend(std::iter::repeat_n(0.0f32, 4 * symbol_len));

        // 2-tap multipath: direct + a 0.5 ms echo (24 samples), like the Good channel.
        let delay = 24usize;
        let mut rx = vec![0.0f32; clean.len()];
        for k in 0..clean.len() {
            let echo = if k >= delay {
                0.6 * clean[k - delay]
            } else {
                0.0
            };
            rx[k] = 0.8 * clean[k] + echo;
        }

        let (_v, offset) = detect_coppa_sync(&rx, &profile)
            .expect("autocorrelation sync should detect the preamble under multipath");
        // Timing within the cyclic prefix is good enough for OFDM.
        let err = (offset as i64 - lead as i64).abs();
        assert!(
            err <= profile.cp_samples as i64,
            "offset {offset} should be within CP ({}) of {lead}, err={err}",
            profile.cp_samples
        );
    }
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
    fn test_schmidl_cox_detects_repeated_pattern() {
        let fft_size = 64;
        let cp_len = 16;

        let half: Vec<f32> = (0..fft_size / 2).map(|i| (i as f32 * 0.3).sin()).collect();

        let mut samples = vec![0.0f32; 100];
        for i in (fft_size - cp_len)..fft_size {
            let idx = if i < fft_size / 2 {
                i
            } else {
                i - fft_size / 2
            };
            samples.push(half[idx]);
        }
        samples.extend_from_slice(&half);
        samples.extend_from_slice(&half);
        samples.extend(vec![0.0f32; 100]);

        let detector = SchmidlCox::new(fft_size, cp_len).with_threshold(0.5);
        let result = detector.detect(&samples, 8000.0);
        assert!(result.is_some(), "Should detect the repeated pattern");
    }

    #[test]
    fn test_schmidl_cox_no_sync_in_silence() {
        let fft_size = 64;
        let cp_len = 16;
        let samples = vec![0.0f32; 500];
        let detector = SchmidlCox::new(fft_size, cp_len);
        let result = detector.detect(&samples, 8000.0);
        assert!(result.is_none(), "Should not detect sync in silence");
    }

    #[test]
    fn test_schmidl_cox_too_short_input() {
        let fft_size = 64;
        let cp_len = 16;
        let samples = vec![1.0f32; 10];
        let detector = SchmidlCox::new(fft_size, cp_len);
        let result = detector.detect(&samples, 8000.0);
        assert!(result.is_none(), "Should return None for too-short input");
    }

    #[test]
    fn test_schmidl_cox_timing_offset() {
        let fft_size = 64;
        let cp_len = 16;
        let half: Vec<f32> = (0..fft_size / 2).map(|i| (i as f32 * 0.5).sin()).collect();

        let offset = 200;
        let mut samples = vec![0.0f32; offset];
        for i in (fft_size - cp_len)..fft_size {
            let idx = if i < fft_size / 2 {
                i
            } else {
                i - fft_size / 2
            };
            samples.push(half[idx]);
        }
        samples.extend_from_slice(&half);
        samples.extend_from_slice(&half);
        samples.extend(vec![0.0f32; 100]);

        let detector = SchmidlCox::new(fft_size, cp_len).with_threshold(0.5);
        let result = detector.detect(&samples, 8000.0);
        assert!(result.is_some(), "Should detect pattern at offset");
        let (pos, _cfo) = result.unwrap();
        // Detected position should be near the offset (within CP range)
        assert!(
            (pos as i64 - offset as i64).unsigned_abs() < (fft_size + cp_len) as u64,
            "Detected position {} should be near offset {}",
            pos,
            offset
        );
    }

    #[test]
    fn test_schmidl_cox_cfo_zero_for_real_signal() {
        let fft_size = 64;
        let cp_len = 16;
        let half: Vec<f32> = (0..fft_size / 2).map(|i| (i as f32 * 0.3).sin()).collect();

        let mut samples = vec![0.0f32; 50];
        for i in (fft_size - cp_len)..fft_size {
            let idx = if i < fft_size / 2 {
                i
            } else {
                i - fft_size / 2
            };
            samples.push(half[idx]);
        }
        samples.extend_from_slice(&half);
        samples.extend_from_slice(&half);
        samples.extend(vec![0.0f32; 50]);

        let detector = SchmidlCox::new(fft_size, cp_len).with_threshold(0.5);
        if let Some((_pos, cfo)) = detector.detect(&samples, 8000.0) {
            // Real-valued signal: CFO is always exactly 0
            assert_eq!(cfo, 0.0, "CFO should be exactly zero for real signal");
        }
    }

    #[test]
    fn test_schmidl_cox_complex_cfo_estimation() {
        let fft_size = 64;
        let cp_len = 16;
        let half_len = fft_size / 2;

        // Generate complex STS with two identical halves
        let half: Vec<Complex32> = (0..half_len)
            .map(|i| Complex32::new((i as f32 * 0.3).sin(), (i as f32 * 0.3).cos()))
            .collect();

        let mut samples = vec![Complex32::new(0.0, 0.0); 50];
        // CP
        for i in (fft_size - cp_len)..fft_size {
            let idx = if i < half_len { i } else { i - half_len };
            samples.push(half[idx]);
        }
        samples.extend_from_slice(&half);
        samples.extend_from_slice(&half);
        samples.extend(vec![Complex32::new(0.0, 0.0); 50]);

        // Apply a known CFO
        let cfo_hz = 50.0f32;
        let sample_rate = 8000.0f32;
        for (n, s) in samples.iter_mut().enumerate() {
            let phase = 2.0 * std::f32::consts::PI * cfo_hz * n as f32 / sample_rate;
            *s *= Complex32::new(phase.cos(), phase.sin());
        }

        let detector = SchmidlCox::new(fft_size, cp_len).with_threshold(0.3);
        let result = detector.detect_complex(&samples, sample_rate);
        assert!(result.is_some(), "Should detect complex STS");
        let (_pos, estimated_cfo) = result.unwrap();
        assert!(
            (estimated_cfo.abs() - cfo_hz.abs()).abs() < 20.0,
            "CFO estimate magnitude {} should be near {} Hz",
            estimated_cfo.abs(),
            cfo_hz.abs()
        );
    }

    #[test]
    fn test_schmidl_cox_detects_through_noise() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let fft_size = 64;
        let cp_len = 16;
        let half: Vec<f32> = (0..fft_size / 2).map(|i| (i as f32 * 0.3).sin()).collect();

        let mut samples = vec![0.0f32; 100];
        for i in (fft_size - cp_len)..fft_size {
            let idx = if i < fft_size / 2 {
                i
            } else {
                i - fft_size / 2
            };
            samples.push(half[idx]);
        }
        samples.extend_from_slice(&half);
        samples.extend_from_slice(&half);
        samples.extend(vec![0.0f32; 100]);

        // Compute signal power only over the STS region
        let sts_start = 100;
        let sts_end = sts_start + cp_len + fft_size;
        let signal_power: f32 = samples[sts_start..sts_end]
            .iter()
            .map(|s| s * s)
            .sum::<f32>()
            / (sts_end - sts_start) as f32;
        let noise_power = signal_power / 10.0; // 10 dB SNR
        let noise_std = noise_power.sqrt();
        let mut rng = StdRng::seed_from_u64(42);
        for s in &mut samples {
            let u1: f32 = rng.random::<f32>().max(1e-10);
            let u2: f32 = rng.random();
            let noise =
                noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
            *s += noise;
        }

        let detector = SchmidlCox::new(fft_size, cp_len).with_threshold(0.3);
        let result = detector.detect(&samples, 8000.0);
        assert!(
            result.is_some(),
            "Should detect STS through 10 dB SNR noise"
        );
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

    #[test]
    fn test_coppa_version_detect_v1() {
        let profile = CoppaProfile::hf_standard();
        let preamble = generate_coppa_preamble(&profile, 1);

        // Embed preamble in silence
        let mut samples = vec![0.0f32; 200];
        samples.extend_from_slice(&preamble);
        samples.extend(vec![0.0f32; 200]);

        let result = detect_coppa_version(&samples, &profile);
        assert!(result.is_some(), "Should detect version 1 preamble");
        let (version, _offset) = result.unwrap();
        assert_eq!(version, 1, "Detected version should be 1");
    }

    #[test]
    fn test_coppa_version_detect_v1_not_v2() {
        let profile = CoppaProfile::hf_standard();
        let preamble_v1 = generate_coppa_preamble(&profile, 1);

        // Embed v1 preamble in silence
        let mut samples = vec![0.0f32; 200];
        samples.extend_from_slice(&preamble_v1);
        samples.extend(vec![0.0f32; 200]);

        let corr_v1 = coppa_version_correlation(&samples, &profile, 1);
        let corr_v2 = coppa_version_correlation(&samples, &profile, 2);

        assert!(
            corr_v1 > corr_v2 + 0.1,
            "V1 correlation ({}) should be significantly higher than V2 ({})",
            corr_v1,
            corr_v2
        );
    }
}
