//! Metrics: aggregate per-trial outcomes into FER, BER, and goodput.

use crate::scenario::SAMPLE_RATE;
use coppa_protocol::modem::transceiver::ReceiveError;

/// 95% Wilson score interval for a binomial proportion (errors out of trials).
/// Returns (lo, hi) clamped to [0, 1]. Zero trials → (0.0, 1.0).
pub fn wilson_ci95(errors: usize, trials: usize) -> (f64, f64) {
    if trials == 0 {
        return (0.0, 1.0);
    }
    let n = trials as f64;
    let p = errors as f64 / n;
    let z = 1.959963984540054f64;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denom;
    let half = z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / denom;
    let lo = (center - half).max(0.0);
    // When errors == 0, center and half are mathematically identical (both reduce to
    // z^2/(2n)/denom), but are computed via different arithmetic paths (a direct
    // division for `center` vs. a sqrt-of-a-product for `half`), so they can differ by
    // a ULP-level residual (~1e-18-1e-17) instead of cancelling to exactly 0. That
    // residual is positive, so `.max(0.0)` above doesn't clamp it. Snap it to a clean
    // 0.0 so a zero-error measurement reports an exact lower bound.
    let lo = if lo > 0.0 && lo < 1e-14 { 0.0 } else { lo };
    let hi = (center + half).min(1.0);
    (lo, hi)
}

/// Failure category produced by a receive attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureMode {
    SyncFailed,
    HeaderCorrupt,
    LdpcNotConverged,
    CrcMismatch,
    WrongPayload,
}

impl From<&ReceiveError> for FailureMode {
    fn from(error: &ReceiveError) -> Self {
        match error {
            ReceiveError::SyncFailed => Self::SyncFailed,
            ReceiveError::HeaderCorrupt => Self::HeaderCorrupt,
            ReceiveError::LdpcNotConverged { .. } => Self::LdpcNotConverged,
            ReceiveError::CrcMismatch => Self::CrcMismatch,
        }
    }
}

/// Counts successful trials and each distinct receive failure mode.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FailureCounts {
    pub correct: usize,
    pub wrong_payload: usize,
    pub sync_failed: usize,
    pub header_corrupt: usize,
    pub ldpc_not_converged: usize,
    pub crc_mismatch: usize,
}

impl FailureCounts {
    pub fn record(&mut self, result: Result<bool, &ReceiveError>) {
        match result {
            Ok(true) => self.correct += 1,
            Ok(false) => self.wrong_payload += 1,
            Err(ReceiveError::SyncFailed) => self.sync_failed += 1,
            Err(ReceiveError::HeaderCorrupt) => self.header_corrupt += 1,
            Err(ReceiveError::LdpcNotConverged { .. }) => self.ldpc_not_converged += 1,
            Err(ReceiveError::CrcMismatch) => self.crc_mismatch += 1,
        }
    }

    pub fn trials(self) -> usize {
        self.correct
            + self.wrong_payload
            + self.sync_failed
            + self.header_corrupt
            + self.ldpc_not_converged
            + self.crc_mismatch
    }

    pub fn frame_errors(self) -> usize {
        self.trials() - self.correct
    }

    pub fn fer(self) -> f64 {
        self.frame_errors() as f64 / self.trials().max(1) as f64
    }

    fn record_outcome(&mut self, outcome: &TrialOutcome) {
        if outcome.success {
            self.correct += 1;
            return;
        }
        match outcome.failure {
            Some(FailureMode::SyncFailed) => self.sync_failed += 1,
            Some(FailureMode::HeaderCorrupt) => self.header_corrupt += 1,
            Some(FailureMode::LdpcNotConverged) => self.ldpc_not_converged += 1,
            Some(FailureMode::CrcMismatch) => self.crc_mismatch += 1,
            Some(FailureMode::WrongPayload) => self.wrong_payload += 1,
            None => self.wrong_payload += 1,
        }
    }
}

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
    /// Failure category, absent on a successful trial.
    pub failure: Option<FailureMode>,
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
    /// 95% Wilson interval on FER.
    pub fer_lo: f64,
    pub fer_hi: f64,
    pub ber: f64,
    pub goodput_bps: f64,
    pub failures: FailureCounts,
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
    let (fer_lo, fer_hi) = wilson_ci95(frame_errors, trials);

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
    let mut failures = FailureCounts::default();
    for outcome in outcomes {
        failures.record_outcome(outcome);
    }

    MeasurementPoint {
        level,
        mode_name,
        channel,
        snr_db,
        trials,
        frame_errors,
        fer,
        fer_lo,
        fer_hi,
        ber,
        goodput_bps,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_protocol::modem::transceiver::ReceiveError;

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
                comparable: true,
                failure: None
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
                comparable: false,
                failure: Some(FailureMode::SyncFailed)
            };
            5
        ];
        let p = aggregate(2, "BPSK 1/2", "awgn", -20.0, 121, 48_000, &outcomes);
        assert_eq!(p.fer, 1.0);
        assert_eq!(p.goodput_bps, 0.0);
    }

    #[test]
    fn wilson_ci95_matches_known_values() {
        // 0 errors in 50 trials: upper bound ≈ 0.0713 (z=1.96 Wilson).
        let (lo, hi) = wilson_ci95(0, 50);
        assert_eq!(lo, 0.0);
        assert!((hi - 0.0713).abs() < 0.002, "hi={hi}");
        // 25/50: symmetric around ~0.5.
        let (lo, hi) = wilson_ci95(25, 50);
        assert!((lo - 0.366).abs() < 0.005, "lo={lo}");
        assert!((hi - 0.634).abs() < 0.005, "hi={hi}");
        // Degenerate: no trials → maximally uninformative.
        assert_eq!(wilson_ci95(0, 0), (0.0, 1.0));
    }

    #[test]
    fn aggregate_populates_fer_ci() {
        let outcomes = vec![
            TrialOutcome {
                success: true,
                bit_errors: 0,
                comparable: true,
                failure: None
            };
            50
        ];
        let p = aggregate(2, "BPSK 1/2", "awgn", 10.0, 121, 48_000, &outcomes);
        assert_eq!(p.fer_lo, 0.0);
        assert!(
            p.fer_hi > 0.05 && p.fer_hi < 0.09,
            "0/50 upper CI ~0.071, got {}",
            p.fer_hi
        );
    }

    #[test]
    fn failure_counts_cover_every_receive_error_variant() {
        let mut c = FailureCounts::default();
        c.record(Ok(true));
        c.record(Ok(false));
        c.record(Err(&ReceiveError::SyncFailed));
        c.record(Err(&ReceiveError::HeaderCorrupt));
        c.record(Err(&ReceiveError::LdpcNotConverged { iterations: 30 }));
        c.record(Err(&ReceiveError::CrcMismatch));
        assert_eq!(c.trials(), 6);
        assert_eq!(c.frame_errors(), 5);
    }

    #[test]
    fn aggregate_tallies_failure_modes_per_bucket() {
        let outcomes = vec![
            TrialOutcome {
                success: true,
                bit_errors: 0,
                comparable: true,
                failure: None,
            },
            TrialOutcome {
                success: false,
                bit_errors: 0,
                comparable: false,
                failure: Some(FailureMode::SyncFailed),
            },
            TrialOutcome {
                success: false,
                bit_errors: 0,
                comparable: false,
                failure: Some(FailureMode::LdpcNotConverged),
            },
            TrialOutcome {
                success: false,
                bit_errors: 0,
                comparable: false,
                failure: Some(FailureMode::LdpcNotConverged),
            },
        ];
        let p = aggregate(
            9,
            "64-QAM 2/3",
            "watterson-good",
            30.0,
            196,
            48_000,
            &outcomes,
        );
        assert_eq!(p.failures.sync_failed, 1);
        assert_eq!(p.failures.ldpc_not_converged, 2);
        assert_eq!(p.failures.header_corrupt, 0);
    }

    #[test]
    fn failure_buckets_sum_to_frame_errors() {
        let outcomes = vec![
            TrialOutcome {
                success: true,
                bit_errors: 0,
                comparable: true,
                failure: None,
            },
            TrialOutcome {
                success: false,
                bit_errors: 3,
                comparable: true,
                failure: Some(FailureMode::WrongPayload),
            },
            TrialOutcome {
                success: false,
                bit_errors: 0,
                comparable: false,
                failure: Some(FailureMode::CrcMismatch),
            },
        ];
        let p = aggregate(
            9,
            "64-QAM 2/3",
            "watterson-poor",
            30.0,
            196,
            48_000,
            &outcomes,
        );
        assert_eq!(p.failures.frame_errors(), p.frame_errors);
    }
}
