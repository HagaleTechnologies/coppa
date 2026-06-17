//! Root-Raised Cosine (RRC) pulse-shaping filter.
//!
//! When applied on both TX and RX, the combined response is a raised cosine
//! with zero intersymbol interference at the optimal sampling instants.
use std::f32::consts::PI;

/// Normalization mode for the RRC filter coefficients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NormMode {
    /// Normalize so the DC gain is unity. Appropriate for NRZ-style input
    /// where the filter acts as a spectral shaper (TX path).
    DcGain,
    /// Normalize by sqrt(sum(h^2)) so the filter has unit energy.
    /// Appropriate for matched filtering on the RX path with impulse-train input.
    Energy,
}

pub struct RrcFilter {
    coefficients: Vec<f32>,
}

impl RrcFilter {
    /// Create an RRC filter with DC-gain normalization (backward compatible).
    pub fn new(alpha: f32, span_symbols: usize, samples_per_symbol: usize) -> Self {
        Self::new_with_norm(alpha, span_symbols, samples_per_symbol, NormMode::DcGain)
    }

    /// Create an RRC filter with the specified normalization mode.
    pub fn new_with_norm(
        alpha: f32,
        span_symbols: usize,
        samples_per_symbol: usize,
        norm_mode: NormMode,
    ) -> Self {
        let sps = samples_per_symbol as f32;
        let half_len = (span_symbols * samples_per_symbol) / 2;
        let len = 2 * half_len + 1;
        let mut coefficients = Vec::with_capacity(len);

        for i in 0..len {
            let n = i as f32 - half_len as f32;
            let t = n / sps;
            let h = rrc_impulse(t, alpha);
            coefficients.push(h);
        }

        match norm_mode {
            NormMode::DcGain => {
                // Normalize by DC gain for unit passthrough of constant input.
                let dc_gain: f32 = coefficients.iter().sum();
                if dc_gain.abs() > 1e-10 {
                    for c in &mut coefficients {
                        *c /= dc_gain;
                    }
                }
            }
            NormMode::Energy => {
                // Normalize by sqrt(sum(h^2)) for unit-energy matched filter.
                let energy: f32 = coefficients.iter().map(|h| h * h).sum();
                let norm = energy.sqrt();
                if norm > 1e-10 {
                    for c in &mut coefficients {
                        *c /= norm;
                    }
                }
            }
        }

        Self { coefficients }
    }

    pub fn filter(&self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }

        let half = self.coefficients.len() / 2;
        let mut output = Vec::with_capacity(input.len());

        for i in 0..input.len() {
            let mut sum = 0.0f32;
            for (k, &coeff) in self.coefficients.iter().enumerate() {
                let input_idx = i as isize + k as isize - half as isize;
                if input_idx >= 0 && (input_idx as usize) < input.len() {
                    sum += coeff * input[input_idx as usize];
                }
            }
            output.push(sum);
        }

        output
    }

    pub fn coefficients(&self) -> &[f32] {
        &self.coefficients
    }
}

fn rrc_impulse(t: f32, alpha: f32) -> f32 {
    let eps = 1e-7;

    if t.abs() < eps {
        1.0 - alpha + 4.0 * alpha / PI
    } else if (t.abs() - 1.0 / (4.0 * alpha)).abs() < eps && alpha > eps {
        (alpha / 2.0_f32.sqrt())
            * ((1.0 + 2.0 / PI) * (PI / (4.0 * alpha)).sin()
                + (1.0 - 2.0 / PI) * (PI / (4.0 * alpha)).cos())
    } else {
        let pi_t = PI * t;
        let num = (pi_t * (1.0 - alpha)).sin() + 4.0 * alpha * t * (pi_t * (1.0 + alpha)).cos();
        let den = pi_t * (1.0 - (4.0 * alpha * t).powi(2));
        if den.abs() < eps {
            1.0 - alpha + 4.0 * alpha / PI
        } else {
            num / den
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rrc_filter_creation() {
        let f = RrcFilter::new(0.35, 6, 256);
        assert_eq!(f.coefficients.len(), 6 * 256 + 1);
    }

    #[test]
    fn test_rrc_filter_symmetry() {
        let f = RrcFilter::new(0.35, 6, 256);
        let n = f.coefficients.len();
        for i in 0..n / 2 {
            assert!(
                (f.coefficients[i] - f.coefficients[n - 1 - i]).abs() < 1e-6,
                "RRC filter should be symmetric"
            );
        }
    }

    #[test]
    fn test_rrc_combined_zero_isi() {
        let sps = 32;
        let f = RrcFilter::new(0.35, 6, sps);

        let mut impulse = vec![0.0f32; sps * 12 + 1];
        impulse[sps * 6] = 1.0;

        let once = f.filter(&impulse);
        let twice = f.filter(&once);

        let center = sps * 6;
        for k in 1..=3 {
            let idx = center + k * sps;
            assert!(
                twice[idx].abs() < 0.01,
                "Combined RRC should have near-zero at {} symbol offset, got {}",
                k,
                twice[idx]
            );
            let idx = center - k * sps;
            assert!(
                twice[idx].abs() < 0.01,
                "Combined RRC should have near-zero at -{} symbol offset, got {}",
                k,
                twice[idx]
            );
        }
    }

    #[test]
    fn test_rrc_preserves_length() {
        let f = RrcFilter::new(0.35, 4, 64);
        let input = vec![1.0; 1000];
        let output = f.filter(&input);
        assert_eq!(output.len(), input.len());
    }
}
