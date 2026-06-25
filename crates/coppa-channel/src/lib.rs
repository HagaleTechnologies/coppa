//! Channel models for testing signal impairments.
//!
//! Provides AWGN, single-sideband frequency offset, fractional timing offset,
//! and a deterministic sinusoidal amplitude fade.
//!
//! Note: the [`fading`] function is a simple deterministic sinusoidal amplitude
//! fade (periodic AM), NOT a statistical HF/VHF channel model. It is intended
//! for exercising AGC tracking, not for evaluating performance over a realistic
//! ionospheric channel. For a realistic Rayleigh/Watterson HF channel model,
//! see the [`watterson`] module.

pub mod watterson;

use std::f32::consts::TAU;

/// Add white Gaussian noise at the specified SNR (in dB).
///
/// Uses `rand::thread_rng()` for non-deterministic noise. For deterministic
/// tests, use [`awgn_seeded`] instead.
pub fn awgn(samples: &[f32], snr_db: f32) -> Vec<f32> {
    let mut rng = rand::rng();
    awgn_with_rng(samples, snr_db, &mut rng)
}

/// Add white Gaussian noise with a seeded RNG for deterministic tests.
pub fn awgn_seeded(samples: &[f32], snr_db: f32, seed: u64) -> Vec<f32> {
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    let mut rng = StdRng::seed_from_u64(seed);
    awgn_with_rng(samples, snr_db, &mut rng)
}

fn awgn_with_rng<R: rand::Rng>(samples: &[f32], snr_db: f32, rng: &mut R) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let signal_power = samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32;
    let noise_power = signal_power / 10.0f32.powf(snr_db / 10.0);
    let noise_std = noise_power.sqrt();

    samples
        .iter()
        .map(|&s| {
            // Box-Muller transform for Gaussian noise
            let u1: f32 = (1.0 - rng.random::<f32>()).max(1e-10);
            let theta: f32 = rng.random_range(0.0f32..TAU);
            let noise = noise_std * (-2.0 * u1.ln()).sqrt() * theta.cos();
            s + noise
        })
        .collect()
}

/// Apply a true single-sideband frequency shift to a passband signal.
pub fn freq_offset(
    samples: &[f32],
    offset_hz: f32,
    sample_rate: f32,
    carrier_freq: f32,
) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let cycle_samples = (sample_rate / carrier_freq).round().max(1.0) as usize;
    let mut i_raw = Vec::with_capacity(samples.len());
    let mut q_raw = Vec::with_capacity(samples.len());

    for (n, &s) in samples.iter().enumerate() {
        let phase = TAU * carrier_freq * n as f32 / sample_rate;
        i_raw.push(2.0 * s * phase.cos());
        q_raw.push(-2.0 * s * phase.sin());
    }

    let i_bb = moving_average(&i_raw, cycle_samples);
    let q_bb = moving_average(&q_raw, cycle_samples);

    let mut output = Vec::with_capacity(samples.len());
    for n in 0..samples.len() {
        let t = n as f32 / sample_rate;
        let shift_phase = TAU * offset_hz * t;
        let (shift_cos, shift_sin) = (shift_phase.cos(), shift_phase.sin());

        let i_shifted = i_bb[n] * shift_cos - q_bb[n] * shift_sin;
        let q_shifted = i_bb[n] * shift_sin + q_bb[n] * shift_cos;

        let carrier_phase = TAU * carrier_freq * t;
        output.push(i_shifted * carrier_phase.cos() - q_shifted * carrier_phase.sin());
    }

    output
}

/// Apply a fractional sample delay using windowed sinc interpolation.
pub fn timing_offset(samples: &[f32], delay_samples: f32) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let n = samples.len();
    let mut output = Vec::with_capacity(n);
    let half_window = 16;

    for i in 0..n {
        let src = i as f32 - delay_samples;
        let mut sum = 0.0f32;

        for k in -half_window..=half_window {
            let idx = src.round() as isize + k as isize;
            if idx >= 0 && (idx as usize) < n {
                let delta = src - idx as f32;
                let w = sinc(delta) * blackman_harris(k as f32, half_window as f32);
                sum += samples[idx as usize] * w;
            }
        }

        output.push(sum);
    }

    output
}

/// Apply a deterministic sinusoidal amplitude fade (periodic AM).
///
/// This is NOT Rayleigh, Watterson, or any Doppler-spread fading model. It
/// multiplies the signal by a single low-frequency cosine that swings between
/// full amplitude and `-depth_db` down, with the swing rate set by
/// `fade_rate_hz`. It is useful only for testing AGC tracking against a smooth,
/// predictable amplitude variation. Modeling realistic HF/VHF fading would
/// require a statistical model (e.g. Rayleigh or Watterson), which this crate
/// does not implement.
///
/// ```
/// let samples = vec![1.0f32; 8000];
/// // 1 Hz fade, 20 dB deep, at 8 kHz sample rate.
/// let faded = coppa_channel::fading(&samples, 1.0, 20.0, 8000.0);
/// assert_eq!(faded.len(), samples.len());
/// // The fade dips well below the original amplitude somewhere in the buffer.
/// let min = faded.iter().cloned().fold(f32::MAX, f32::min);
/// assert!(min < 0.5);
/// ```
pub fn fading(samples: &[f32], fade_rate_hz: f32, depth_db: f32, sample_rate: f32) -> Vec<f32> {
    let depth_linear = 10.0f32.powf(-depth_db / 20.0);

    samples
        .iter()
        .enumerate()
        .map(|(n, &s)| {
            let t = n as f32 / sample_rate;
            let fade = 0.5 * (1.0 + depth_linear)
                + 0.5 * (1.0 - depth_linear) * (TAU * fade_rate_hz * t).cos();
            s * fade
        })
        .collect()
}

fn moving_average(samples: &[f32], window: usize) -> Vec<f32> {
    let window = window.max(1);
    let mut output = Vec::with_capacity(samples.len());
    let mut sum = 0.0f32;

    for i in 0..samples.len() {
        sum += samples[i];
        if i >= window {
            sum -= samples[i - window];
        }
        let count = (i + 1).min(window);
        output.push(sum / count as f32);
    }

    output
}

fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-7 {
        1.0
    } else {
        let px = std::f32::consts::PI * x;
        px.sin() / px
    }
}

fn blackman_harris(k: f32, half_window: f32) -> f32 {
    let x = (k + half_window) / (2.0 * half_window);
    let pi2x = TAU * x;
    0.35875 - 0.48829 * pi2x.cos() + 0.14128 * (2.0 * pi2x).cos() - 0.01168 * (3.0 * pi2x).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_awgn_preserves_length() {
        let samples = vec![1.0, -1.0, 1.0, -1.0];
        let noisy = awgn(&samples, 10.0);
        assert_eq!(noisy.len(), samples.len());
    }

    #[test]
    fn test_awgn_empty() {
        assert!(awgn(&[], 10.0).is_empty());
    }

    #[test]
    fn test_awgn_high_snr_preserves_sign() {
        let samples = vec![1.0; 100];
        let noisy = awgn(&samples, 40.0);
        for s in &noisy {
            assert!(*s > 0.5, "High SNR should preserve signal sign");
        }
    }

    #[test]
    fn test_freq_offset_preserves_length() {
        let samples = vec![1.0; 100];
        let shifted = freq_offset(&samples, 5.0, 8000.0, 1000.0);
        assert_eq!(shifted.len(), samples.len());
    }

    #[test]
    fn test_freq_offset_is_true_shift() {
        let sr = 8000.0;
        let fc = 1000.0;
        let n = 2048;
        let tone: Vec<f32> = (0..n).map(|i| (TAU * fc * i as f32 / sr).cos()).collect();

        let shifted = freq_offset(&tone, 50.0, sr, fc);

        let ref_1050: Vec<f32> = (0..n)
            .map(|i| (TAU * 1050.0 * i as f32 / sr).cos())
            .collect();
        let ref_950: Vec<f32> = (0..n)
            .map(|i| (TAU * 950.0 * i as f32 / sr).cos())
            .collect();

        let start = n / 2;
        let corr_1050: f32 =
            (start..n).map(|i| shifted[i] * ref_1050[i]).sum::<f32>() / (n - start) as f32;
        let corr_950: f32 =
            (start..n).map(|i| shifted[i] * ref_950[i]).sum::<f32>() / (n - start) as f32;

        assert!(
            corr_1050.abs() > corr_950.abs() * 3.0,
            "Should be a true shift, not DSB-AM. corr_1050={}, corr_950={}",
            corr_1050,
            corr_950
        );
    }

    #[test]
    fn test_timing_offset_preserves_length() {
        let samples: Vec<f32> = (0..200).map(|i| (i as f32 * 0.1).sin()).collect();
        let delayed = timing_offset(&samples, 3.7);
        assert_eq!(delayed.len(), samples.len());
    }

    #[test]
    fn test_fading_preserves_length() {
        let samples = vec![1.0; 100];
        let faded = fading(&samples, 1.0, 10.0, 8000.0);
        assert_eq!(faded.len(), samples.len());
    }

    #[test]
    fn test_fading_reduces_amplitude() {
        let samples = vec![1.0; 8000];
        let faded = fading(&samples, 1.0, 20.0, 8000.0);
        let min = faded.iter().cloned().fold(f32::MAX, f32::min);
        assert!(min < 0.5, "Fading should reduce amplitude at some points");
    }
}
