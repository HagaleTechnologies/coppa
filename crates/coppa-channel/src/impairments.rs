//! Synthetic finite-buffer impairments. Inputs must contain only finite samples.
use coppa_dsp::{agc::AdaptiveAgc, fft::FftProcessor};
use num_complex::Complex32;
use rand::{rngs::StdRng, RngExt, SeedableRng};

/// Add independent Bernoulli impulses with random signs, in input amplitude units.
/// Probability must be finite in [0, 1], amplitude finite and nonnegative.
/// Invalid configuration returns `None`, including on empty input. No normalization.
pub fn impulsive_noise_seeded(
    samples: &[f32],
    probability: f32,
    amplitude: f32,
    seed: u64,
) -> Option<Vec<f32>> {
    if !probability.is_finite()
        || !(0.0..=1.0).contains(&probability)
        || !amplitude.is_finite()
        || amplitude < 0.0
    {
        return None;
    }
    let mut rng = StdRng::seed_from_u64(seed);
    Some(
        samples
            .iter()
            .map(|&x| {
                if rng.random::<f32>() < probability {
                    x + if rng.random::<bool>() {
                        amplitude
                    } else {
                        -amplitude
                    }
                } else {
                    x
                }
            })
            .collect(),
    )
}

/// Symmetric hard limiting in input amplitude units. Threshold must be finite and
/// positive. Returns `None` for invalid configuration, even on empty input.
pub fn hard_clip(samples: &[f32], threshold: f32) -> Option<Vec<f32>> {
    if !threshold.is_finite() || threshold <= 0.0 {
        return None;
    }
    Some(
        samples
            .iter()
            .map(|x| x.clamp(-threshold, threshold))
            .collect(),
    )
}

/// Fresh synthetic block AGC, target in linear RMS units, block size in samples.
/// Target must be finite and positive and block size nonzero, even on empty input.
/// Uses AdaptiveAgc defaults: attack 0.6, release 0.85, gain 1e-4 through 1e4.
pub fn agc(samples: &[f32], target_rms: f32, block_samples: usize) -> Option<Vec<f32>> {
    if !target_rms.is_finite() || target_rms <= 0.0 || block_samples == 0 {
        return None;
    }
    Some(AdaptiveAgc::new(target_rms, block_samples).process(samples))
}

/// Zero-phase finite-buffer spectral shaping with circular boundary behavior.
/// This is not a causal rig filter or group-delay model. Tilt is the dB difference
/// from 300 to 2700 Hz, anchored at unity at 1500 Hz and clamped outside the band.
/// Tilt must be finite within +/-60 dB; sample rate must be finite and >5400 Hz.
/// Invalid configuration returns `None` even for empty input. No normalization.
pub fn frequency_tilt(samples: &[f32], tilt_db: f32, sample_rate: f32) -> Option<Vec<f32>> {
    if !tilt_db.is_finite()
        || tilt_db.abs() > 60.0
        || !sample_rate.is_finite()
        || sample_rate <= 5400.0
    {
        return None;
    }
    if samples.is_empty() || tilt_db == 0.0 {
        return Some(samples.to_vec());
    }
    let fft = FftProcessor::new(samples.len());
    let mut bins = fft.forward(
        &samples
            .iter()
            .map(|&x| Complex32::new(x, 0.0))
            .collect::<Vec<_>>(),
    );
    let n = bins.len();
    for (k, bin) in bins.iter_mut().enumerate() {
        let hz = k.min(n - k) as f32 * sample_rate / n as f32;
        let db = tilt_db * (hz.clamp(300.0, 2700.0) - 1500.0) / 2400.0;
        *bin *= 10.0_f32.powf(db / 20.0);
    }
    Some(fft.inverse(&bins).iter().map(|x| x.re).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn impulses_are_seeded_and_signed() {
        let x = vec![0.0; 4096];
        assert_eq!(impulsive_noise_seeded(&x, 0.0, 3.0, 1), Some(x.clone()));
        assert_eq!(impulsive_noise_seeded(&x, 1.0, 0.0, 1), Some(x.clone()));
        let a = impulsive_noise_seeded(&x, 1.0, 3.0, 1).unwrap();
        assert!(a.iter().all(|v| v.abs() == 3.0));
        assert_eq!(Some(a.clone()), impulsive_noise_seeded(&x, 1.0, 3.0, 1));
        assert_ne!(Some(a), impulsive_noise_seeded(&x, 1.0, 3.0, 2));
        for p in [-1.0, 1.1, f32::NAN, f32::INFINITY] {
            assert!(impulsive_noise_seeded(&[], p, 1.0, 1).is_none());
        }
        for a in [-1.0, f32::NAN, f32::INFINITY] {
            assert!(impulsive_noise_seeded(&[], 0.1, a, 1).is_none());
        }
    }
    #[test]
    fn clipping_and_domains() {
        assert_eq!(
            hard_clip(&[-2.0, -0.5, 0.5, 2.0], 1.0),
            Some(vec![-1.0, -0.5, 0.5, 1.0])
        );
        for v in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(hard_clip(&[], v).is_none());
            assert!(agc(&[], v, 4).is_none());
        }
        assert!(agc(&[], 1.0, 0).is_none());
    }
    #[test]
    fn agc_tracks_with_existing_attack_release() {
        let x: Vec<_> = [vec![0.1; 8], vec![2.0; 8], vec![0.1; 8]].concat();
        let y = agc(&x, 0.5, 4).unwrap();
        assert_eq!(y, AdaptiveAgc::new(0.5, 4).process(&x));
        assert!(y[8] > y[12]);
        assert!(y[16] < y[20]);
        assert_eq!(agc(&[0.0; 20], 0.5, 4), Some(vec![0.0; 20]));
    }
    #[test]
    fn tilt_tones_and_domains() {
        for n in [48000, 48001] {
            for (f, db) in [(300.0, -6.0), (1500.0, 0.0), (2700.0, 6.0)] {
                let x: Vec<_> = (0..n)
                    .map(|i| (std::f32::consts::TAU * f * i as f32 / 48000.0).sin())
                    .collect();
                assert_eq!(frequency_tilt(&x, 0.0, 48000.0), Some(x.clone()));
                for sign in [-1.0, 1.0] {
                    let y = frequency_tilt(&x, sign * 12.0, 48000.0).unwrap();
                    assert_eq!(y.len(), n);
                    assert!(y.iter().all(|v| v.is_finite()));
                    let ratio = 10.0 * (crate::mean_power(&y) / crate::mean_power(&x)).log10();
                    assert!((ratio - sign * db).abs() < 0.02, "{ratio} {db}");
                }
            }
        }
        for rate in [0.0, 5400.0, f32::NAN, f32::INFINITY] {
            assert!(frequency_tilt(&[], 0.0, rate).is_none());
        }
        for tilt in [-61.0, 61.0, f32::NAN, f32::INFINITY] {
            assert!(frequency_tilt(&[], tilt, 48000.0).is_none());
        }
    }
    #[test]
    fn valid_empty_inputs() {
        assert_eq!(impulsive_noise_seeded(&[], 0.1, 1.0, 1), Some(vec![]));
        assert_eq!(hard_clip(&[], 1.0), Some(vec![]));
        assert_eq!(agc(&[], 0.5, 4), Some(vec![]));
        assert_eq!(frequency_tilt(&[], 12.0, 48000.0), Some(vec![]));
    }
}
