//! OFDM channel equalization.
//!
//! Implements MMSE equalization using pilot-based channel estimates.
use crate::traits::ChannelEstimator;
use num_complex::Complex32;

/// Linear interpolation channel estimator.
///
/// Estimates channel response at pilot positions, then interpolates
/// to get estimates at all data subcarrier positions.
pub struct LinearInterpolationEstimator {
    /// Channel estimates at each subcarrier index.
    h_estimates: Vec<Complex32>,
    /// Number of active subcarriers.
    num_carriers: usize,
    /// Estimated noise variance.
    noise_var: f32,
}

impl LinearInterpolationEstimator {
    pub fn new(num_carriers: usize) -> Self {
        Self {
            h_estimates: vec![Complex32::new(1.0, 0.0); num_carriers],
            num_carriers,
            noise_var: 0.01,
        }
    }

    /// Compute per-carrier effective noise variance after MMSE equalization.
    ///
    /// For each data carrier k, the effective noise variance on the equalized
    /// symbol is σ²/|H[k]|². This accounts for frequency-selective fading:
    /// carriers with weak channel gain have higher effective noise.
    pub fn per_carrier_noise(&self, data_carrier_indices: &[usize]) -> Vec<f32> {
        let sigma2 = self.noise_var;
        data_carrier_indices
            .iter()
            .map(|&k| {
                let h_sq = if k < self.h_estimates.len() {
                    self.h_estimates[k].norm_sqr()
                } else {
                    1.0
                };
                if h_sq > 1e-10 {
                    sigma2 / h_sq
                } else {
                    sigma2 * 1e6
                }
            })
            .collect()
    }

    /// Per-carrier Wiener gain g[k] = |H[k]|^2 / (|H[k]|^2 + sigma^2) — the amplitude bias the
    /// MMSE equalizer applies. Dividing an equalized symbol by g un-biases it to constellation
    /// scale (equivalent to zero-forcing), which matters for amplitude-bearing QAM.
    pub fn per_carrier_gain(&self, data_carrier_indices: &[usize]) -> Vec<f32> {
        let sigma2 = self.noise_var;
        data_carrier_indices
            .iter()
            .map(|&k| {
                let h_sq = if k < self.h_estimates.len() {
                    self.h_estimates[k].norm_sqr()
                } else {
                    1.0
                };
                (h_sq / (h_sq + sigma2)).max(1e-6)
            })
            .collect()
    }
}

impl ChannelEstimator for LinearInterpolationEstimator {
    fn update(&mut self, pilots: &[(usize, Complex32, Complex32)]) {
        if pilots.is_empty() {
            return;
        }

        // Compute H at pilot positions: H[k] = Y[k] / X_known[k]
        let mut pilot_estimates: Vec<(usize, Complex32)> = pilots
            .iter()
            .filter(|(_, _, known)| known.norm() > 1e-10)
            .map(|&(idx, received, known)| (idx, received / known))
            .collect();

        pilot_estimates.sort_by_key(|(idx, _)| *idx);

        if pilot_estimates.is_empty() {
            return;
        }

        // Estimate noise variance from pilot prediction error:
        // |received_pilot - H_est * known_pilot|^2
        // This gives the true noise variance for the MMSE denominator,
        // rather than tracking channel variation between updates.
        let mut noise_sum = 0.0f32;
        let mut noise_count = 0usize;
        for &(idx, received, known) in pilots {
            if idx < self.num_carriers && known.norm() > 1e-10 {
                let predicted = self.h_estimates[idx] * known;
                let residual = (received - predicted).norm_sqr();
                noise_sum += residual;
                noise_count += 1;
            }
        }
        if noise_count > 0 {
            self.noise_var = (noise_sum / noise_count as f32).max(1e-10);
        }

        // Linear interpolation between pilot positions
        for i in 0..pilot_estimates.len() {
            let (idx_a, h_a) = pilot_estimates[i];
            if idx_a < self.num_carriers {
                self.h_estimates[idx_a] = h_a;
            }

            if i + 1 < pilot_estimates.len() {
                let (idx_b, h_b) = pilot_estimates[i + 1];
                // Interpolate between idx_a and idx_b
                for k in (idx_a + 1)..idx_b {
                    if k < self.num_carriers {
                        let frac = (k - idx_a) as f32 / (idx_b - idx_a) as f32;
                        self.h_estimates[k] = h_a * (1.0 - frac) + h_b * frac;
                    }
                }
            }
        }

        // Extrapolate beyond the last pilot
        if let Some(&(last_idx, last_h)) = pilot_estimates.last() {
            for k in (last_idx + 1)..self.num_carriers {
                self.h_estimates[k] = last_h;
            }
        }
        // Extrapolate before the first pilot
        if let Some(&(first_idx, first_h)) = pilot_estimates.first() {
            for k in 0..first_idx {
                self.h_estimates[k] = first_h;
            }
        }
    }

    fn estimate(&self, subcarrier: usize) -> Complex32 {
        if subcarrier < self.h_estimates.len() {
            self.h_estimates[subcarrier]
        } else {
            Complex32::new(1.0, 0.0)
        }
    }

    fn noise_variance(&self) -> f32 {
        self.noise_var
    }
}

/// MMSE equalizer: X_hat[k] = Y[k] * H*[k] / (|H[k]|^2 + sigma^2)
pub fn mmse_equalize(
    received: &[Complex32],
    estimator: &dyn ChannelEstimator,
    num_carriers: usize,
) -> Vec<Complex32> {
    let noise_var = estimator.noise_variance();
    let mut equalized = Vec::with_capacity(num_carriers.min(received.len()));

    for (k, &rx) in received.iter().take(num_carriers).enumerate() {
        let h = estimator.estimate(k);
        let h_conj = h.conj();
        let h_sq = h.norm_sqr();
        let denom = h_sq + noise_var;
        if denom > 1e-10 {
            equalized.push(rx * h_conj / denom);
        } else {
            equalized.push(rx);
        }
    }

    equalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimator_flat_channel() {
        let mut est = LinearInterpolationEstimator::new(16);
        // Flat channel: H = 1+0j everywhere
        let pilots: Vec<(usize, Complex32, Complex32)> = vec![
            (2, Complex32::new(1.0, 0.0), Complex32::new(1.0, 0.0)),
            (6, Complex32::new(1.0, 0.0), Complex32::new(1.0, 0.0)),
            (10, Complex32::new(1.0, 0.0), Complex32::new(1.0, 0.0)),
            (14, Complex32::new(1.0, 0.0), Complex32::new(1.0, 0.0)),
        ];
        est.update(&pilots);

        for k in 0..16 {
            let h = est.estimate(k);
            assert!(
                (h - Complex32::new(1.0, 0.0)).norm() < 0.1,
                "Channel estimate at {} should be ~1, got {:?}",
                k,
                h
            );
        }
    }

    #[test]
    fn test_mmse_equalize_identity() {
        let est = LinearInterpolationEstimator::new(4);
        let received = vec![
            Complex32::new(1.0, 0.0),
            Complex32::new(-1.0, 0.0),
            Complex32::new(1.0, 0.0),
            Complex32::new(-1.0, 0.0),
        ];
        let equalized = mmse_equalize(&received, &est, 4);
        for (orig, eq) in received.iter().zip(equalized.iter()) {
            assert!(
                (orig - eq).norm() < 0.1,
                "MMSE with flat channel should preserve signal"
            );
        }
    }

    #[test]
    fn test_estimator_frequency_selective_channel() {
        let mut est = LinearInterpolationEstimator::new(16);
        // Channel with varying response: H[2]=2, H[6]=0.5, H[10]=1.5, H[14]=0.8
        let pilots: Vec<(usize, Complex32, Complex32)> = vec![
            (2, Complex32::new(2.0, 0.0), Complex32::new(1.0, 0.0)),
            (6, Complex32::new(0.5, 0.0), Complex32::new(1.0, 0.0)),
            (10, Complex32::new(1.5, 0.0), Complex32::new(1.0, 0.0)),
            (14, Complex32::new(0.8, 0.0), Complex32::new(1.0, 0.0)),
        ];
        est.update(&pilots);

        // Check pilot positions are accurate
        assert!((est.estimate(2) - Complex32::new(2.0, 0.0)).norm() < 0.01);
        assert!((est.estimate(6) - Complex32::new(0.5, 0.0)).norm() < 0.01);
        assert!((est.estimate(10) - Complex32::new(1.5, 0.0)).norm() < 0.01);
        assert!((est.estimate(14) - Complex32::new(0.8, 0.0)).norm() < 0.01);

        // Interpolated positions should be between neighbors
        let h4 = est.estimate(4);
        assert!(
            h4.re > 0.5 && h4.re < 2.0,
            "Interpolated H[4] should be between H[2] and H[6], got {}",
            h4.re
        );
    }

    #[test]
    fn test_estimator_out_of_range() {
        let est = LinearInterpolationEstimator::new(8);
        // Out-of-range subcarrier should return default (1,0)
        let h = est.estimate(100);
        assert_eq!(h, Complex32::new(1.0, 0.0));
    }

    #[test]
    fn test_estimator_empty_pilots() {
        let mut est = LinearInterpolationEstimator::new(8);
        // Empty update should not crash
        est.update(&[]);
        let h = est.estimate(0);
        assert_eq!(h, Complex32::new(1.0, 0.0));
    }

    #[test]
    fn test_mmse_equalize_with_channel() {
        // Simulate a channel where H = 2+0j at all subcarriers
        let mut est = LinearInterpolationEstimator::new(4);
        let pilots: Vec<(usize, Complex32, Complex32)> = vec![
            (0, Complex32::new(2.0, 0.0), Complex32::new(1.0, 0.0)),
            (3, Complex32::new(2.0, 0.0), Complex32::new(1.0, 0.0)),
        ];
        est.update(&pilots);

        // Transmitted [1, -1, 1, -1], received through H=2: [2, -2, 2, -2]
        let received = vec![
            Complex32::new(2.0, 0.0),
            Complex32::new(-2.0, 0.0),
            Complex32::new(2.0, 0.0),
            Complex32::new(-2.0, 0.0),
        ];
        let equalized = mmse_equalize(&received, &est, 4);

        // Should recover approximately [1, -1, 1, -1]
        for (i, eq) in equalized.iter().enumerate() {
            let expected = if i % 2 == 0 { 1.0 } else { -1.0 };
            assert!(
                (eq.re - expected).abs() < 0.2,
                "Subcarrier {}: expected ~{}, got {}",
                i,
                expected,
                eq.re
            );
        }
    }

    #[test]
    fn test_per_carrier_noise_frequency_selective() {
        let mut est = LinearInterpolationEstimator::new(8);
        let pilots: Vec<(usize, Complex32, Complex32)> = vec![
            (0, Complex32::new(2.0, 0.0), Complex32::new(1.0, 0.0)),
            (4, Complex32::new(0.5, 0.0), Complex32::new(1.0, 0.0)),
            (7, Complex32::new(1.0, 0.0), Complex32::new(1.0, 0.0)),
        ];
        est.update(&pilots);

        let data_indices: Vec<usize> = (0..8).collect();
        let noise = est.per_carrier_noise(&data_indices);

        assert!(
            noise[0] < noise[4],
            "Carrier 0 (strong) should have lower noise than carrier 4 (weak): {} vs {}",
            noise[0],
            noise[4]
        );
        for (i, &n) in noise.iter().enumerate() {
            assert!(
                n > 0.0,
                "Noise at carrier {} should be positive, got {}",
                i,
                n
            );
        }
    }

    #[test]
    fn test_equalize_frequency_selective_with_noise() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let num_carriers = 16;
        let mut est = LinearInterpolationEstimator::new(num_carriers);

        // Frequency-selective channel: H varies across subcarriers
        let h_values = [
            2.0f32, 1.5, 1.0, 0.8, 0.5, 0.7, 1.2, 1.8, 2.0, 1.5, 1.0, 0.8, 0.5, 0.7, 1.2, 1.8,
        ];

        // Pilot positions with known transmitted values
        let pilots: Vec<(usize, Complex32, Complex32)> = vec![
            (
                0,
                Complex32::new(h_values[0], 0.0),
                Complex32::new(1.0, 0.0),
            ),
            (
                4,
                Complex32::new(h_values[4], 0.0),
                Complex32::new(1.0, 0.0),
            ),
            (
                8,
                Complex32::new(h_values[8], 0.0),
                Complex32::new(1.0, 0.0),
            ),
            (
                12,
                Complex32::new(h_values[12], 0.0),
                Complex32::new(1.0, 0.0),
            ),
        ];
        est.update(&pilots);

        // Transmitted BPSK: alternating ±1
        let transmitted: Vec<Complex32> = (0..num_carriers)
            .map(|i| Complex32::new(if i % 2 == 0 { 1.0 } else { -1.0 }, 0.0))
            .collect();

        // Received: Y = H * X + noise
        let mut rng = StdRng::seed_from_u64(99);
        let noise_std = 0.1;
        let received: Vec<Complex32> = (0..num_carriers)
            .map(|i| {
                let u1: f32 = rng.random::<f32>().max(1e-10);
                let u2: f32 = rng.random();
                let n_re =
                    noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                Complex32::new(h_values[i], 0.0) * transmitted[i] + Complex32::new(n_re, 0.0)
            })
            .collect();

        let equalized = mmse_equalize(&received, &est, num_carriers);

        // Check that equalized values are close to transmitted
        let mut correct = 0;
        for (i, eq) in equalized.iter().enumerate() {
            let decision = if eq.re >= 0.0 { 1.0 } else { -1.0 };
            if (decision - transmitted[i].re).abs() < 0.01 {
                correct += 1;
            }
        }
        assert!(
            correct >= 12, // Allow a few errors from interpolation inaccuracy
            "At least 12/16 subcarriers should be correctly equalized, got {}",
            correct
        );
    }
}
