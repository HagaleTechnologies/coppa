//! Metrics: aggregate per-trial outcomes into FER, BER, and goodput.

use crate::scenario::SAMPLE_RATE;

/// Outcome of a single transmit → channel → receive trial.
#[derive(Debug, Clone, Copy)]
pub struct TrialOutcome {
    /// Whether the exact payload was recovered.
    pub success: bool,
    /// Post-decode bit errors on this trial (0 on clean success).
    pub bit_errors: usize,
    /// Whether the receiver produced a payload to compare (true) or failed to
    /// decode entirely (false). BER is averaged only over comparable trials.
    pub comparable: bool,
}

/// Aggregated measurement at one (mode, channel, SNR) point.
#[derive(Debug, Clone)]
pub struct MeasurementPoint {
    pub level: u8,
    pub mode_name: &'static str,
    pub channel: &'static str,
    pub snr_db: f32,
    pub trials: usize,
    pub frame_errors: usize,
    pub fer: f64,
    pub ber: f64,
    pub goodput_bps: f64,
}

/// Count differing bits between two byte slices (Hamming distance), comparing
/// up to the shorter length.
pub fn bit_errors(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones() as usize)
        .sum()
}

/// Aggregate trial outcomes into a `MeasurementPoint`.
///
/// - `payload_bytes`: payload carried per frame for this mode.
/// - `frame_samples`: audio samples in one transmitted frame (for airtime).
#[allow(clippy::too_many_arguments)]
pub fn aggregate(
    level: u8,
    mode_name: &'static str,
    channel: &'static str,
    snr_db: f32,
    payload_bytes: usize,
    frame_samples: usize,
    outcomes: &[TrialOutcome],
) -> MeasurementPoint {
    let trials = outcomes.len();
    let frame_errors = outcomes.iter().filter(|o| !o.success).count();
    let fer = if trials > 0 {
        frame_errors as f64 / trials as f64
    } else {
        0.0
    };

    let comparable = outcomes.iter().filter(|o| o.comparable).count();
    let payload_bits = payload_bytes * 8;
    let total_bits = comparable * payload_bits;
    let total_bit_errors: usize = outcomes
        .iter()
        .filter(|o| o.comparable)
        .map(|o| o.bit_errors)
        .sum();
    let ber = if total_bits > 0 {
        total_bit_errors as f64 / total_bits as f64
    } else {
        0.0
    };

    let frame_airtime_s = frame_samples as f64 / SAMPLE_RATE as f64;
    let goodput_bps = if frame_airtime_s > 0.0 {
        payload_bits as f64 * (1.0 - fer) / frame_airtime_s
    } else {
        0.0
    };

    MeasurementPoint {
        level,
        mode_name,
        channel,
        snr_db,
        trials,
        frame_errors,
        fer,
        ber,
        goodput_bps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_errors_counts_differing_bits() {
        assert_eq!(bit_errors(&[0xFF], &[0x0F]), 4);
        assert_eq!(bit_errors(&[0x00, 0x00], &[0x00, 0x00]), 0);
    }

    #[test]
    fn all_success_gives_zero_fer_and_positive_goodput() {
        let outcomes = vec![
            TrialOutcome {
                success: true,
                bit_errors: 0,
                comparable: true
            };
            10
        ];
        let p = aggregate(2, "BPSK 1/2", "awgn", 30.0, 121, 48_000, &outcomes);
        assert_eq!(p.fer, 0.0);
        assert_eq!(p.ber, 0.0);
        assert!((p.goodput_bps - 968.0).abs() < 1e-6);
    }

    #[test]
    fn all_failure_gives_unit_fer_and_zero_goodput() {
        let outcomes = vec![
            TrialOutcome {
                success: false,
                bit_errors: 0,
                comparable: false
            };
            5
        ];
        let p = aggregate(2, "BPSK 1/2", "awgn", -20.0, 121, 48_000, &outcomes);
        assert_eq!(p.fer, 1.0);
        assert_eq!(p.goodput_bps, 0.0);
    }
}
