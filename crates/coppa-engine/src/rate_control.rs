//! SNR/FER-based rate controller with hysteresis.

/// Rate controller that adapts MCS based on SNR and frame error rate.
pub struct RateController {
    /// Current MCS index.
    current_mcs: u8,
    /// Minimum MCS index (most robust).
    min_mcs: u8,
    /// Maximum MCS index (fastest).
    max_mcs: u8,
    /// SNR threshold to upgrade (hysteresis high).
    upgrade_snr_margin: f32,
    /// SNR threshold to downgrade (hysteresis low).
    downgrade_snr_margin: f32,
    /// FER threshold to trigger downgrade.
    fer_threshold: f32,
    /// Smoothed SNR estimate.
    smoothed_snr: f32,
    /// Smoothed FER estimate.
    smoothed_fer: f32,
    /// EWMA alpha for SNR.
    snr_alpha: f32,
    /// EWMA alpha for FER.
    fer_alpha: f32,
    /// Number of consecutive frames at current MCS (for stability).
    stability_counter: usize,
    /// Minimum frames before allowing MCS change.
    stability_threshold: usize,
    /// SNR thresholds for each MCS index (from MCS table).
    mcs_snr_thresholds: Vec<f32>,
}

impl RateController {
    /// Create a new rate controller.
    ///
    /// * `initial_mcs` - Starting MCS index
    /// * `min_mcs` - Minimum (most robust) MCS
    /// * `max_mcs` - Maximum (fastest) MCS
    pub fn new(initial_mcs: u8, min_mcs: u8, max_mcs: u8) -> Self {
        // Default SNR thresholds matching the MCS table
        let mcs_snr_thresholds = vec![-2.0, 0.0, 2.0, 5.0, 7.0, 10.0, 12.0, 15.0, 16.0, 20.0, 25.0];

        Self {
            current_mcs: initial_mcs.clamp(min_mcs, max_mcs),
            min_mcs,
            max_mcs,
            upgrade_snr_margin: 3.0,
            downgrade_snr_margin: 1.0,
            fer_threshold: 0.1,
            smoothed_snr: 10.0,
            smoothed_fer: 0.0,
            snr_alpha: 0.2,
            fer_alpha: 0.3,
            stability_counter: 0,
            stability_threshold: 10,
            mcs_snr_thresholds,
        }
    }

    /// Set hysteresis margins for MCS transitions.
    pub fn set_hysteresis(&mut self, upgrade_margin: f32, downgrade_margin: f32) {
        self.upgrade_snr_margin = upgrade_margin;
        self.downgrade_snr_margin = downgrade_margin;
    }

    /// Set the FER threshold that triggers a downgrade.
    pub fn set_fer_threshold(&mut self, threshold: f32) {
        self.fer_threshold = threshold;
    }

    /// Set the stability threshold (minimum frames before MCS change).
    pub fn set_stability_threshold(&mut self, frames: usize) {
        self.stability_threshold = frames;
    }

    /// Update the controller with a new SNR measurement and frame success/failure.
    ///
    /// Returns the (possibly updated) MCS index.
    pub fn update(&mut self, snr_db: f32, frame_success: bool) -> u8 {
        // Update EWMA estimates
        self.smoothed_snr = self.snr_alpha * snr_db + (1.0 - self.snr_alpha) * self.smoothed_snr;
        let fer_sample = if frame_success { 0.0 } else { 1.0 };
        self.smoothed_fer =
            self.fer_alpha * fer_sample + (1.0 - self.fer_alpha) * self.smoothed_fer;

        self.stability_counter += 1;

        // Don't change MCS until stability threshold is met
        if self.stability_counter < self.stability_threshold {
            return self.current_mcs;
        }

        // Check for downgrade conditions
        if self.smoothed_fer > self.fer_threshold {
            self.downgrade();
            return self.current_mcs;
        }

        let current_threshold = self
            .mcs_snr_thresholds
            .get(self.current_mcs as usize)
            .copied()
            .unwrap_or(0.0);

        // Check for downgrade: SNR dropped below current MCS threshold minus margin
        if self.smoothed_snr < current_threshold - self.downgrade_snr_margin {
            self.downgrade();
            return self.current_mcs;
        }

        // Check for upgrade: SNR exceeds next MCS threshold plus margin
        if self.current_mcs < self.max_mcs {
            let next_threshold = self
                .mcs_snr_thresholds
                .get(self.current_mcs as usize + 1)
                .copied()
                .unwrap_or(f32::MAX);

            if self.smoothed_snr > next_threshold + self.upgrade_snr_margin
                && self.smoothed_fer < self.fer_threshold * 0.5
            {
                self.upgrade();
            }
        }

        self.current_mcs
    }

    /// Force an upgrade to the next MCS level.
    fn upgrade(&mut self) {
        if self.current_mcs < self.max_mcs {
            self.current_mcs += 1;
            self.stability_counter = 0;
        }
    }

    /// Force a downgrade to a lower MCS level.
    fn downgrade(&mut self) {
        if self.current_mcs > self.min_mcs {
            self.current_mcs -= 1;
            self.stability_counter = 0;
        }
    }

    /// Get the current MCS index.
    pub fn current_mcs(&self) -> u8 {
        self.current_mcs
    }

    /// Get the current smoothed SNR estimate.
    pub fn smoothed_snr(&self) -> f32 {
        self.smoothed_snr
    }

    /// Get the current smoothed FER estimate.
    pub fn smoothed_fer(&self) -> f32 {
        self.smoothed_fer
    }

    /// Reset the controller to its initial state with the given MCS.
    pub fn reset(&mut self, mcs: u8) {
        self.current_mcs = mcs.clamp(self.min_mcs, self.max_mcs);
        self.smoothed_snr = 10.0;
        self.smoothed_fer = 0.0;
        self.stability_counter = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_controller_initial_state() {
        let rc = RateController::new(2, 0, 10);
        assert_eq!(rc.current_mcs(), 2);
    }

    #[test]
    fn test_rate_controller_stability() {
        let mut rc = RateController::new(2, 0, 10);
        // High SNR but not enough frames for stability
        for _ in 0..5 {
            rc.update(30.0, true);
        }
        assert_eq!(rc.current_mcs(), 2); // Should not change yet
    }

    #[test]
    fn test_rate_controller_upgrade() {
        let mut rc = RateController::new(2, 0, 10);
        rc.set_stability_threshold(5);

        // Feed high SNR with all successes
        for _ in 0..20 {
            rc.update(30.0, true);
        }

        assert!(
            rc.current_mcs() > 2,
            "Should have upgraded from MCS 2, got {}",
            rc.current_mcs()
        );
    }

    #[test]
    fn test_rate_controller_downgrade_on_fer() {
        let mut rc = RateController::new(5, 0, 10);
        rc.set_stability_threshold(5);

        // Feed failures
        for _ in 0..20 {
            rc.update(10.0, false);
        }

        assert!(
            rc.current_mcs() < 5,
            "Should have downgraded from MCS 5, got {}",
            rc.current_mcs()
        );
    }

    #[test]
    fn test_rate_controller_downgrade_on_low_snr() {
        let mut rc = RateController::new(5, 0, 10);
        rc.set_stability_threshold(5);

        // Feed low SNR
        for _ in 0..20 {
            rc.update(-5.0, true);
        }

        assert!(rc.current_mcs() < 5, "Should have downgraded on low SNR");
    }

    #[test]
    fn test_rate_controller_clamp() {
        let mut rc = RateController::new(0, 0, 10);
        rc.set_stability_threshold(1);

        // Try to downgrade below minimum
        for _ in 0..20 {
            rc.update(-10.0, false);
        }
        assert_eq!(rc.current_mcs(), 0);
    }

    #[test]
    fn test_rate_controller_reset() {
        let mut rc = RateController::new(5, 0, 10);
        rc.set_stability_threshold(5);
        for _ in 0..20 {
            rc.update(30.0, true);
        }
        let upgraded_mcs = rc.current_mcs();
        assert!(upgraded_mcs > 5);

        rc.reset(2);
        assert_eq!(rc.current_mcs(), 2);
        assert_eq!(rc.smoothed_fer(), 0.0);
    }

    #[test]
    fn test_rate_controller_hysteresis() {
        let mut rc = RateController::new(4, 0, 10);
        rc.set_stability_threshold(3);
        rc.set_hysteresis(5.0, 2.0); // Wide upgrade margin

        // SNR slightly above next threshold - not enough for upgrade
        // MCS 4 threshold is 7.0, MCS 5 threshold is 10.0
        // Need > 10.0 + 5.0 = 15.0 for upgrade
        for _ in 0..20 {
            rc.update(12.0, true);
        }
        // With 5.0 upgrade margin, 12.0 < 10.0 + 5.0 = 15.0, so should not upgrade
        assert_eq!(rc.current_mcs(), 4, "Should NOT upgrade with hysteresis");
    }

    #[test]
    fn test_rate_controller_smoothing() {
        let mut rc = RateController::new(2, 0, 10);
        rc.update(20.0, true);
        let snr1 = rc.smoothed_snr();
        rc.update(20.0, true);
        let snr2 = rc.smoothed_snr();
        // EWMA should converge toward 20
        assert!(snr2 > snr1 || (snr2 - 20.0).abs() < 0.1);
    }
}
