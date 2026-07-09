//! Sender-side closed-loop rate controller. Applies the receiver's per-frame speed-level
//! recommendation (fed back on the ACK) with hysteresis, plus an ARQ-failure safety override:
//! **raise slow** (step up one level only after `raise_dwell` consecutive higher recommendations),
//! **drop fast** (a lower recommendation, or a delivery failure, applies immediately). Steps through
//! the ordered set of valid speed levels, so reserved level 8 is skipped.

/// Ordered ascending valid coppa speed levels (level 8 is reserved / excluded).
pub const VALID_SPEED_LEVELS: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 9, 10];

/// Sender-side adaptive rate controller. Holds an index into an ascending level set.
pub struct RateLoop {
    levels: Vec<u8>,
    idx: usize,
    raise_dwell: u8,
    raise_run: u8,
}

impl RateLoop {
    /// `levels` must be ascending and non-empty; `initial_level` is clamped into the set.
    /// `raise_dwell` is the number of consecutive higher recommendations required to step up.
    pub fn new(levels: Vec<u8>, raise_dwell: u8, initial_level: u8) -> Self {
        assert!(!levels.is_empty(), "RateLoop needs a non-empty level set");
        let idx = Self::rank(&levels, initial_level);
        Self {
            levels,
            idx,
            raise_dwell: raise_dwell.max(1),
            raise_run: 0,
        }
    }

    /// The standard coppa level set, dwell 3, starting at the most robust level.
    pub fn default_coppa() -> Self {
        Self::new(VALID_SPEED_LEVELS.to_vec(), 3, 1)
    }

    /// Index of the highest level `<= level` (clamped into range).
    fn rank(levels: &[u8], level: u8) -> usize {
        let mut idx = 0;
        for (i, &l) in levels.iter().enumerate() {
            if l <= level {
                idx = i;
            } else {
                break;
            }
        }
        idx
    }

    /// Current speed level to transmit at.
    pub fn current_level(&self) -> u8 {
        self.levels[self.idx]
    }

    /// Apply one ACK. `feedback_level` is the receiver's recommendation; `delivered` is whether the
    /// acked frame was correctly received.
    pub fn on_ack(&mut self, feedback_level: u8, delivered: bool) {
        if !delivered {
            self.idx = self.idx.saturating_sub(1);
            self.raise_run = 0;
            return;
        }
        let fb = Self::rank(&self.levels, feedback_level);
        if fb < self.idx {
            self.idx = fb; // channel worsened -> drop immediately to the recommendation
            self.raise_run = 0;
        } else if fb > self.idx {
            self.raise_run += 1;
            if self.raise_run >= self.raise_dwell {
                self.idx = (self.idx + 1).min(self.levels.len() - 1); // raise slow, one step
                self.raise_run = 0;
            }
        } else {
            self.raise_run = 0; // hold
        }
    }

    /// A retransmit timeout occurred — hard failure signal, drop fast. Per the ARQ integration
    /// pattern (`ArqTx::get_retransmits`), one timeout EVENT (any number of expired segments in a
    /// single poll) should map to exactly one call here, not one call per expired segment.
    pub fn on_timeout(&mut self) {
        self.idx = self.idx.saturating_sub(1);
        self.raise_run = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_initial_level() {
        assert_eq!(RateLoop::default_coppa().current_level(), 1);
        assert_eq!(
            RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 3, 6).current_level(),
            6
        );
    }

    #[test]
    fn raise_is_slow_and_one_step() {
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 3, 1);
        r.on_ack(10, true); // higher recommendation, run=1
        assert_eq!(r.current_level(), 1);
        r.on_ack(10, true); // run=2
        assert_eq!(r.current_level(), 1);
        r.on_ack(10, true); // run=3 -> step up ONE level
        assert_eq!(r.current_level(), 2);
        r.on_ack(10, true); // run=1 again after reset
        assert_eq!(r.current_level(), 2);
    }

    #[test]
    fn drop_is_immediate_to_recommendation() {
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 3, 7);
        r.on_ack(3, true); // lower recommendation -> jump straight to 3
        assert_eq!(r.current_level(), 3);
    }

    #[test]
    fn failure_drops_one_step() {
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 3, 6);
        r.on_ack(10, false); // not delivered -> drop one step (6 -> 5)
        assert_eq!(r.current_level(), 5);
        r.on_timeout(); // 5 -> 4
        assert_eq!(r.current_level(), 4);
    }

    #[test]
    fn skips_reserved_level_8() {
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 1, 7);
        r.on_ack(10, true); // dwell 1 -> step up from 7; next valid level is 9, NOT 8
        assert_eq!(r.current_level(), 9);
    }

    #[test]
    fn clamps_at_floor_and_ceiling() {
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 1, 1);
        r.on_ack(1, false); // already at floor
        assert_eq!(r.current_level(), 1);
        let mut r2 = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 1, 10);
        r2.on_ack(10, true); // already at ceiling
        assert_eq!(r2.current_level(), 10);
    }
}
