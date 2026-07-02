//! Watterson HF channel model: a tapped-delay line with independent Rayleigh-faded
//! taps and Gaussian Doppler spread, after ITU-R F.1487 / CCIR. Applied to real
//! passband audio via an analytic-signal (FFT Hilbert) representation.
//!
//! This is a statistical ionospheric channel model — unlike the simple sinusoidal
//! `fading` in this crate, it produces genuine multipath + Doppler fading.
//!
//! # How to interpret it
//!
//! - **Block-fading regime.** At HF Doppler spreads (0.1–1 Hz) the channel coherence
//!   time (~1–10 s) is comparable to or longer than a modem frame, so within one frame
//!   the fading is slowly varying; averaging over many seeds samples the Rayleigh
//!   fading distribution, including deep flat fades that cost real SNR.
//! - **`doppler_spread_hz` is the ITU-R F.1487 frequency spread (the two-sigma width
//!   of the Gaussian Doppler PSD)**; the PSD sigma is `doppler_spread_hz / 2`.
//! - The fading process is ensemble-normalized (E|g|² = 1): each realization keeps its
//!   Rayleigh amplitude, so per-frame output power varies. Normalizing power
//!   per-realization instead (to exactly 1 every time) would pin every frame's power
//!   and delete flat fading entirely — the whole point of a Rayleigh channel model is
//!   that some frames arrive faded deep and some do not. Set noise from the CLEAN
//!   signal power (see `awgn_ref_seeded`), never from the faded output, or fading
//!   cannot cost SNR.

use coppa_dsp::fft::FftProcessor;
use num_complex::Complex32;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::f32::consts::TAU;

/// One propagation path: relative delay (seconds) and average power (linear).
#[derive(Debug, Clone, Copy)]
pub struct Tap {
    pub delay_s: f32,
    pub power: f32,
}

/// Watterson channel configuration: a set of faded taps and a Doppler spread.
#[derive(Debug, Clone)]
pub struct WattersonConfig {
    pub taps: Vec<Tap>,
    pub doppler_spread_hz: f32,
}

impl WattersonConfig {
    /// Gaussian Doppler PSD sigma. ITU-R F.1487 defines the channel's
    /// "frequency spread" as the two-sigma width of the Doppler spectrum, so
    /// sigma is half the configured spread.
    pub fn doppler_sigma_hz(&self) -> f32 {
        self.doppler_spread_hz * 0.5
    }
}

/// Standard ITU-R F.1487 / CCIR HF test channels (two equal-power paths).
#[derive(Debug, Clone, Copy)]
pub enum WattersonPreset {
    Good,
    Moderate,
    Poor,
}

impl WattersonPreset {
    /// The (delay, Doppler-spread) parameters for this preset.
    pub fn config(self) -> WattersonConfig {
        let (delay_ms, doppler_hz) = match self {
            WattersonPreset::Good => (0.5, 0.1),
            WattersonPreset::Moderate => (1.0, 0.5),
            WattersonPreset::Poor => (2.0, 1.0),
        };
        WattersonConfig {
            taps: vec![
                Tap {
                    delay_s: 0.0,
                    power: 0.5,
                },
                Tap {
                    delay_s: delay_ms / 1000.0,
                    power: 0.5,
                },
            ],
            doppler_spread_hz: doppler_hz,
        }
    }
}

/// A pair of independent N(0,1) samples (Box-Muller).
fn gaussian_pair(rng: &mut impl Rng) -> (f32, f32) {
    let u1: f32 = (1.0 - rng.random::<f32>()).max(1e-12);
    let u2: f32 = rng.random::<f32>();
    let r = (-2.0 * u1.ln()).sqrt();
    (r * (TAU * u2).cos(), r * (TAU * u2).sin())
}

/// Analytic (complex) signal of a real input via an FFT Hilbert transform:
/// keep DC, double the positive-frequency bins, zero the negative ones.
fn analytic(fft: &FftProcessor, x: &[f32]) -> Vec<Complex32> {
    let n = x.len();
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

/// A complex Gaussian fading process of length `n`, band-limited to a Gaussian
/// Doppler PSD with the given **sigma** (`doppler_sigma_hz`), via the spectrum
/// method. Ensemble-normalized: E|g[i]|² = 1 across realizations, while each
/// realization keeps its Rayleigh amplitude statistics (deep fades included).
/// Per-realization normalization would pin every frame's power to 1 and delete
/// flat fading — see the module docs.
fn fading_process(
    fft: &FftProcessor,
    n: usize,
    doppler_sigma_hz: f32,
    fs: f32,
    rng: &mut impl Rng,
) -> Vec<Complex32> {
    let d = doppler_sigma_hz.max(1e-6);
    let mut spec = vec![Complex32::new(0.0, 0.0); n];
    let mut shape_energy = 0.0f32;
    for (k, s) in spec.iter_mut().enumerate() {
        // Bin frequency in [-fs/2, fs/2).
        let f = if k <= n / 2 {
            k as f32
        } else {
            k as f32 - n as f32
        } * fs
            / n as f32;
        let shape = (-0.5 * (f / d).powi(2)).exp();
        shape_energy += shape * shape;
        let (g1, g2) = gaussian_pair(rng);
        *s = Complex32::new(g1 * shape, g2 * shape);
    }
    let mut g = fft.inverse(&spec);
    // E|g[i]|² = (2/N²)·Σ shape² for the 1/N-scaled IFFT of independent
    // CN(0, 2·shape²) bins; divide by sqrt of that deterministic constant.
    let expected_p = 2.0 * shape_energy / (n as f32 * n as f32);
    if expected_p > 1e-30 {
        let scale = (1.0 / expected_p).sqrt();
        for c in &mut g {
            *c *= scale;
        }
    }
    g
}

/// Pass a real passband signal through a Watterson HF channel. Deterministic in
/// `seed`. Output power is preserved only in ensemble average across seeds; each
/// realization fades (use [`crate::awgn_ref_seeded`] with the CLEAN signal's
/// power as reference so that fading costs SNR).
pub fn watterson(
    samples: &[f32],
    sample_rate: f32,
    config: &WattersonConfig,
    seed: u64,
) -> Vec<f32> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    let fft = FftProcessor::new(n);
    let mut rng = StdRng::seed_from_u64(seed);

    let a = analytic(&fft, samples);
    let mut out = vec![Complex32::new(0.0, 0.0); n];
    for tap in &config.taps {
        let delay = (tap.delay_s * sample_rate).round() as usize;
        let amp = tap.power.max(0.0).sqrt();
        let g = fading_process(&fft, n, config.doppler_sigma_hz(), sample_rate, &mut rng);
        for i in delay..n {
            out[i] += a[i - delay] * g[i] * amp;
        }
    }

    // No output renormalization: tap powers sum to 1 and the fading process is
    // ensemble-unit-power, so the ENSEMBLE average output power equals the input
    // power, while each frame keeps its Rayleigh fade (a deep fade arrives quiet).
    out.iter().map(|c| c.re).collect()
}

/// Convenience: pass through a named preset channel.
pub fn watterson_preset(
    samples: &[f32],
    sample_rate: f32,
    preset: WattersonPreset,
    seed: u64,
) -> Vec<f32> {
    watterson(samples, sample_rate, &preset.config(), seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_signal(n: usize) -> Vec<f32> {
        // A 1 kHz-ish tone at 48 kHz, the kind of passband audio the modem produces.
        (0..n).map(|i| (i as f32 * 0.13).sin() * 0.5).collect()
    }

    #[test]
    fn output_length_matches_input() {
        let x = test_signal(4096);
        let y = watterson_preset(&x, 48_000.0, WattersonPreset::Moderate, 1);
        assert_eq!(y.len(), x.len());
    }

    #[test]
    fn ensemble_power_is_preserved() {
        // Per-frame power now varies (Rayleigh); only the ensemble average over
        // seeds must match the input power. 2-tap equal-power channel with a
        // near-constant-per-frame tap: frame power ≈ (|g1|²+|g2|²)/2, a mean-1
        // Gamma(2, 1/2) variable with CV = 1/sqrt(2) ≈ 0.707. Over 200 seeds the
        // std of the mean is ≈ 0.707/sqrt(200) ≈ 0.05 → 0.2 tolerance is 4 sigma.
        let x = test_signal(8192);
        let px = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32;
        let mut ratios = Vec::new();
        for seed in 0..200u64 {
            let y = watterson_preset(&x, 48_000.0, WattersonPreset::Good, seed);
            let py = y.iter().map(|v| v * v).sum::<f32>() / y.len() as f32;
            ratios.push(py / px);
        }
        let mean = ratios.iter().sum::<f32>() / ratios.len() as f32;
        assert!(
            (mean - 1.0).abs() < 0.2,
            "ensemble power ratio should be ~1, got {mean}"
        );
    }

    #[test]
    fn flat_fades_cost_power() {
        // A deep flat fade must be possible: over 200 seeds of a 2-tap channel,
        // at least one frame should arrive at less than 15% of the input power
        // (P(Gamma(2,1/2) < 0.15) ≈ 3.7% per seed → P(none in 200) ≈ 0.05%),
        // and at least one above 2x (P ≈ 9.2% per seed).
        let x = test_signal(8192);
        let px = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32;
        let mut min_ratio = f32::MAX;
        let mut max_ratio = 0.0f32;
        for seed in 0..200u64 {
            let y = watterson_preset(&x, 48_000.0, WattersonPreset::Good, seed);
            let py = y.iter().map(|v| v * v).sum::<f32>() / y.len() as f32;
            min_ratio = min_ratio.min(py / px);
            max_ratio = max_ratio.max(py / px);
        }
        assert!(
            min_ratio < 0.15,
            "deep fades must exist, min ratio {min_ratio}"
        );
        assert!(
            max_ratio > 2.0,
            "up-fades must exist, max ratio {max_ratio}"
        );
    }

    #[test]
    fn doppler_sigma_is_half_the_itu_spread() {
        // ITU-R F.1487 frequency spread = 2 sigma of the Gaussian Doppler PSD.
        let cfg = WattersonPreset::Poor.config(); // spread 1.0 Hz
        assert!((cfg.doppler_sigma_hz() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn fading_process_autocorrelation_matches_gaussian_psd() {
        // The per-bin *amplitude* weight is shape(f) = exp(-0.5(f/sigma)^2), so the
        // power spectral density (E|s_k|^2 ∝ shape(f)^2) is exp(-f^2/sigma^2) — a
        // Gaussian with variance sigma^2/2 in the exp(-f^2/2v) form. Its Fourier
        // pair gives rho(tau) = exp(-2 pi^2 v tau^2) = exp(-pi^2 sigma^2 tau^2).
        // sigma = 0.5 Hz, tau = 0.2 s -> rho ≈ 0.906. (Under the old sigma=spread
        // convention Poor used sigma=1.0 Hz -> rho ≈ 0.674 by the same formula,
        // confirmed empirically at ~0.67 with these n/fs; the two conventions are
        // well separated, so this also guards the 2-sigma convention end to end.)
        // Average over 50 seeds; per-seed estimates are noisy but the mean is tight.
        let n = 48_000; // 1 s at 48 kHz
        let lag = 9_600; // 0.2 s
        let fft = FftProcessor::new(n);
        let mut rho_sum = 0.0f32;
        for seed in 0..50u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let g = fading_process(&fft, n, 0.5, 48_000.0, &mut rng);
            let num: f32 = (0..n - lag).map(|t| (g[t] * g[t + lag].conj()).re).sum();
            let den: f32 = (0..n - lag).map(|t| g[t].norm_sqr()).sum();
            rho_sum += num / den.max(1e-20);
        }
        let rho = rho_sum / 50.0;
        assert!(
            (0.85..0.99).contains(&rho),
            "rho(0.2s) at sigma=0.5 should be ~0.91 (well above the ~0.67 the old \
             sigma=spread convention would give), got {rho}"
        );
    }

    #[test]
    fn deterministic_in_seed() {
        let x = test_signal(4096);
        let a = watterson_preset(&x, 48_000.0, WattersonPreset::Good, 42);
        let b = watterson_preset(&x, 48_000.0, WattersonPreset::Good, 42);
        let c = watterson_preset(&x, 48_000.0, WattersonPreset::Good, 43);
        assert_eq!(a, b, "same seed must give identical output");
        assert_ne!(a, c, "different seed must give different output");
    }

    #[test]
    fn channel_actually_distorts_the_signal() {
        // A faded multipath channel must change the signal, not pass it through.
        // With true (non-renormalized) Rayleigh statistics, a single hardcoded
        // seed can legitimately land near an identity-like channel (a benign
        // fade), so this asserts the MEDIAN distortion over many seeds instead
        // of a single seed's — robust to the occasional benign-fade outlier
        // while still requiring the channel to genuinely distort typically.
        let x = test_signal(8192);
        let energy = x.iter().map(|v| v * v).sum::<f32>();
        let mut ratios: Vec<f32> = (0..20u64)
            .map(|seed| {
                let y = watterson_preset(&x, 48_000.0, WattersonPreset::Poor, seed);
                let diff = x.iter().zip(&y).map(|(a, b)| (a - b).powi(2)).sum::<f32>();
                diff / energy
            })
            .collect();
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = ratios[ratios.len() / 2];
        assert!(
            median > 0.1,
            "channel should distort the signal meaningfully (median), got {median}"
        );
    }

    #[test]
    fn preset_parameters_are_correct() {
        let poor = WattersonPreset::Poor.config();
        assert_eq!(poor.taps.len(), 2);
        assert!((poor.taps[1].delay_s - 0.002).abs() < 1e-9);
        assert!((poor.doppler_spread_hz - 1.0).abs() < 1e-9);
    }

    #[test]
    fn fading_process_has_unit_ensemble_power_and_rayleigh_spread() {
        // At sigma = 0.1 Hz over 8192/48k ≈ 171 ms the frequency resolution
        // (48000/8192 ≈ 5.9 Hz) means only the DC bin has non-negligible shape:
        // the tap is a single complex Gaussian per realization, so per-frame
        // power ~ Exp(1): CV = 1, and P(power < 0.1) ≈ 9.5% → deep fades exist.
        let n = 8192;
        let fft = FftProcessor::new(n);
        let mut powers = Vec::new();
        for seed in 0..300u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let g = fading_process(&fft, n, 0.1, 48_000.0, &mut rng);
            powers.push(g.iter().map(|c| c.norm_sqr()).sum::<f32>() / n as f32);
        }
        let mean = powers.iter().sum::<f32>() / powers.len() as f32;
        // std of the mean of 300 Exp(1) draws ≈ 1/sqrt(300) ≈ 0.058 → 0.2 is >3 sigma.
        assert!(
            (mean - 1.0).abs() < 0.2,
            "ensemble mean power ~1, got {mean}"
        );
        let var = powers.iter().map(|p| (p - mean).powi(2)).sum::<f32>() / powers.len() as f32;
        let cv = var.sqrt() / mean;
        assert!(
            cv > 0.6,
            "per-realization power must vary (Rayleigh), cv={cv}"
        );
        let min = powers.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            min < 0.1 * mean,
            "deep fades must exist, min={min} mean={mean}"
        );
    }
}
