//! Decode-independent channel truth helpers.
//!
//! The frequency response is `H(f,t) = sum sqrt(power) * gain(t) * exp(-i 2 pi f delay)`.
//! This flat-per-carrier model assumes the CP absorbs all multipath, the channel is static during
//! one OFDM symbol, and perfect CSI/sync. It is therefore an optimistic outage bound. In
//! particular, a 60-sample CP does not absorb Watterson-Poor's 96-sample second tap, so callers
//! must not present that arm as an ISI-aware Poor verdict. `nv_ref` may include the transmitter's
//! level-specific PAPR clipping self-noise, as desired for a level-specific question.

use std::f32::consts::TAU;

use coppa_channel::watterson::Tap;
use num_complex::Complex32;

/// Correlation coefficient, with a zero result for degenerate input.
pub fn pearson(xs: &[f32], ys: &[f32]) -> f32 {
    assert_eq!(xs.len(), ys.len());
    if xs.is_empty() {
        return 0.0;
    }
    let n = xs.len() as f32;
    let mx = xs.iter().sum::<f32>() / n;
    let my = ys.iter().sum::<f32>() / n;
    let (mut cov, mut vx, mut vy) = (0.0, 0.0, 0.0);
    for (&x, &y) in xs.iter().zip(ys) {
        let (dx, dy) = (x - mx, y - my);
        cov += dx * dy;
        vx += dx * dx;
        vy += dy * dy;
    }
    cov / (vx.sqrt() * vy.sqrt()).max(1e-9)
}

/// Exact model frequency response for one carrier at one sample.
pub fn per_carrier_gain(
    gains: &[(Tap, Vec<Complex32>)],
    time: usize,
    frequency_hz: f32,
) -> Complex32 {
    gains
        .iter()
        .fold(Complex32::new(0.0, 0.0), |h, (tap, samples)| {
            let phase = -TAU * frequency_hz * tap.delay_s;
            h + samples[time] * tap.power.max(0.0).sqrt() * Complex32::new(phase.cos(), phase.sin())
        })
}

/// Per-carrier perfect-CSI noise variances, averaged over `[start, end)`.
pub fn ground_truth_noise_variances(
    gains: &[(Tap, Vec<Complex32>)],
    start: usize,
    end: usize,
    carrier_freqs: &[f32],
    nv_ref: f32,
) -> Vec<f32> {
    assert!(start < end);
    const TIME_STRIDE: usize = 97;
    carrier_freqs
        .iter()
        .map(|&frequency| {
            let mut h2 = 0.0;
            let mut n = 0;
            for time in (start..end).step_by(TIME_STRIDE) {
                h2 += per_carrier_gain(gains, time, frequency).norm_sqr();
                n += 1;
            }
            nv_ref / (h2 / n as f32).max(1e-9)
        })
        .collect()
}

/// Capacity and selectivity in the same units/formula as `coppa_ml`.
pub fn ground_truth_capacity(
    gains: &[(Tap, Vec<Complex32>)],
    start: usize,
    end: usize,
    carrier_freqs: &[f32],
    nv_ref: f32,
) -> (f32, f32) {
    let nv = ground_truth_noise_variances(gains, start, end, carrier_freqs, nv_ref);
    (
        coppa_ml::channel_capacity(&nv),
        coppa_ml::channel_selectivity(&nv),
    )
}

/// Result of the perfect-CSI isolated-FEC admissibility oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissibilityVerdict {
    pub converged: bool,
    pub payload_matches: bool,
}

impl AdmissibilityVerdict {
    pub fn admissible(self) -> bool {
        self.converged && self.payload_matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_protocol::{
        fec::ldpc::NrLdpc,
        modem::speed_levels::{k_used_for_level, max_payload_for_level},
    };

    fn tap(delay_s: f32, power: f32, gain: Complex32) -> (Tap, Vec<Complex32>) {
        (Tap { delay_s, power }, vec![gain; 200])
    }

    #[test]
    fn single_zero_delay_tap_makes_h_flat_across_carriers() {
        let gains = vec![tap(0.0, 1.0, Complex32::new(0.3, -0.4))];
        assert_eq!(
            per_carrier_gain(&gains, 0, 0.0),
            per_carrier_gain(&gains, 0, 2_500.0)
        );
    }

    #[test]
    fn two_equal_taps_null_at_the_expected_comb_frequency() {
        let gains = vec![
            tap(0.0, 0.5, Complex32::new(1.0, 0.0)),
            tap(0.001, 0.5, Complex32::new(1.0, 0.0)),
        ];
        assert!(per_carrier_gain(&gains, 0, 500.0).norm() < 1e-5);
    }

    #[test]
    fn ground_truth_noise_variance_scales_inversely_with_h_squared() {
        let unit = vec![tap(0.0, 1.0, Complex32::new(1.0, 0.0))];
        let double = vec![tap(0.0, 1.0, Complex32::new(2.0, 0.0))];
        let a = ground_truth_noise_variances(&unit, 0, 100, &[500.0], 0.2)[0];
        let b = ground_truth_noise_variances(&double, 0, 100, &[500.0], 0.2)[0];
        assert!((b - a / 4.0).abs() < 1e-6);
    }

    #[test]
    fn ground_truth_capacity_matches_coppa_ml_on_the_same_nv_array() {
        let gains = vec![tap(0.0, 1.0, Complex32::new(1.0, 0.0))];
        let (capacity, selectivity) = ground_truth_capacity(&gains, 0, 100, &[500.0, 1_000.0], 0.1);
        let nv = [0.1, 0.1];
        assert_eq!(capacity, coppa_ml::channel_capacity(&nv));
        assert_eq!(selectivity, coppa_ml::channel_selectivity(&nv));
    }

    #[test]
    fn level_9_wire_constants_are_pinned() {
        assert_eq!(k_used_for_level(9), Some(1296));
        assert_eq!(max_payload_for_level(9), Some(158));
        assert_eq!(NrLdpc::INFO_LEN, 1760);
        assert_eq!(NrLdpc::MOTHER_LEN, 8800);
    }
}
