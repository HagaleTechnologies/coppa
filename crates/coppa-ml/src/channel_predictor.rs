//! EWMA-based channel quality predictor with trend extrapolation.

use crate::{ChannelPredictor, MlModel};

/// Exponentially Weighted Moving Average channel predictor.
///
/// Tracks channel quality using EWMA smoothing and estimates a linear
/// trend for short-term extrapolation.
pub struct EwmaPredictor {
    /// EWMA smoothing factor (0..1, higher = more weight on recent)
    alpha: f32,
    /// Current smoothed quality estimate.
    smoothed: f32,
    /// Previous smoothed value for trend estimation.
    prev_smoothed: f32,
    /// Estimated trend (quality change per frame).
    trend: f32,
    /// Number of observations received.
    observations: usize,
    /// Confidence based on number of observations.
    confidence_val: f32,
}

impl EwmaPredictor {
    /// Create a new EWMA predictor.
    ///
    /// * `alpha` - Smoothing factor in (0, 1). Higher means more responsive.
    /// * `initial_snr` - Initial SNR estimate in dB.
    pub fn new(alpha: f32, initial_snr: f32) -> Self {
        Self {
            alpha: alpha.clamp(0.01, 0.99),
            smoothed: initial_snr,
            prev_smoothed: initial_snr,
            trend: 0.0,
            observations: 0,
            confidence_val: 0.0,
        }
    }

    /// Get the current smoothed SNR estimate.
    pub fn current_snr(&self) -> f32 {
        self.smoothed
    }

    /// Get the estimated trend (dB per frame).
    pub fn trend(&self) -> f32 {
        self.trend
    }
}

impl MlModel for EwmaPredictor {
    fn model_type(&self) -> &str {
        "ewma-predictor"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn confidence(&self) -> f32 {
        self.confidence_val
    }
}

impl ChannelPredictor for EwmaPredictor {
    fn observe(&mut self, quality: f32) {
        self.prev_smoothed = self.smoothed;
        self.smoothed = self.alpha * quality + (1.0 - self.alpha) * self.smoothed;

        // Update trend with smoothing
        let raw_trend = self.smoothed - self.prev_smoothed;
        self.trend = 0.3 * raw_trend + 0.7 * self.trend;

        self.observations += 1;
        // Confidence ramps up with observations, saturates at ~50
        self.confidence_val = (1.0 - (-0.05 * self.observations as f32).exp()).min(1.0);
    }

    fn predict(&self, frames_ahead: usize) -> f32 {
        // Linear extrapolation from current smoothed value
        self.smoothed + self.trend * frames_ahead as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ewma_initial_state() {
        let pred = EwmaPredictor::new(0.3, 20.0);
        assert_eq!(pred.model_type(), "ewma-predictor");
        assert_eq!(pred.confidence(), 0.0);
        assert_eq!(pred.predict(0), 20.0);
        assert_eq!(pred.predict(10), 20.0); // no trend yet
    }

    #[test]
    fn test_ewma_convergence() {
        let mut pred = EwmaPredictor::new(0.3, 10.0);

        // Feed constant 20 dB observations
        for _ in 0..100 {
            pred.observe(20.0);
        }

        // Should converge to ~20 dB
        assert!(
            (pred.current_snr() - 20.0).abs() < 0.1,
            "SNR didn't converge: {}",
            pred.current_snr()
        );
        assert!(
            pred.confidence() > 0.9,
            "Confidence too low: {}",
            pred.confidence()
        );
    }

    #[test]
    fn test_ewma_step_response() {
        let mut pred = EwmaPredictor::new(0.3, 10.0);

        // Jump to 30 dB
        for _ in 0..50 {
            pred.observe(30.0);
        }

        let snr = pred.current_snr();
        assert!(
            (snr - 30.0).abs() < 1.0,
            "Should converge to 30 dB after step, got {}",
            snr
        );
    }

    #[test]
    fn test_ewma_trend() {
        let mut pred = EwmaPredictor::new(0.3, 10.0);

        // Rising channel: 10, 11, 12, 13...
        for i in 0..20 {
            pred.observe(10.0 + i as f32);
        }

        // Trend should be positive
        assert!(
            pred.trend() > 0.0,
            "Trend should be positive for rising channel"
        );
    }

    #[test]
    fn test_ewma_prediction_with_trend() {
        let mut pred = EwmaPredictor::new(0.3, 10.0);

        for i in 0..30 {
            pred.observe(10.0 + i as f32 * 0.5);
        }

        let now = pred.predict(0);
        let future = pred.predict(5);

        // With positive trend, future should be higher
        assert!(
            future > now,
            "Future prediction should be higher with positive trend"
        );
    }

    #[test]
    fn test_ewma_alpha_clamping() {
        let pred = EwmaPredictor::new(0.0, 10.0); // clamped to 0.01
        assert!((pred.alpha - 0.01).abs() < 0.001);

        let pred = EwmaPredictor::new(1.5, 10.0); // clamped to 0.99
        assert!((pred.alpha - 0.99).abs() < 0.001);
    }
}
