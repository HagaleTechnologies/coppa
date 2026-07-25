//! Sender-side closed-loop rate controller. Applies the receiver's per-frame speed-level
//! recommendation (fed back on the ACK) with hysteresis, plus an ARQ-failure safety override:
//! **raise slow** (step up one level only after `raise_dwell` consecutive higher recommendations),
//! **drop fast** (a lower recommendation, or a delivery failure, applies immediately). Steps through
//! the ordered set of valid speed levels, so reserved level 8 is skipped.
//!
//! Optional **active overshoot probing** (`with_probing`/`level_for_next_transmission`/
//! `on_probe_result`) periodically transmits above the current level on purpose, trading an
//! occasional real decode failure (an ordinary ARQ-retransmitted loss, no protocol change) for
//! much stronger ground truth than the passive per-frame recommendation can give when it's pinned
//! low on a fading channel -- see
//! `docs/superpowers/specs/2026-07-25-rateloop-active-overshoot-probing-design.md` for the full
//! measured rationale.

/// Ordered ascending valid coppa speed levels (level 8 is reserved / excluded).
pub const VALID_SPEED_LEVELS: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 9, 10];

/// Sender-side adaptive rate controller. Holds an index into an ascending level set.
pub struct RateLoop {
    levels: Vec<u8>,
    idx: usize,
    raise_dwell: u8,
    raise_run: u8,
    probe_interval: u32,
    probe_offset: usize,
    frames_since_probe: u32,
}

impl RateLoop {
    /// `levels` must be ascending and non-empty; `initial_level` is clamped into the set.
    /// `raise_dwell` is the number of consecutive higher recommendations required to step up.
    /// Active overshoot probing is disabled by default -- see `with_probing`.
    pub fn new(levels: Vec<u8>, raise_dwell: u8, initial_level: u8) -> Self {
        assert!(!levels.is_empty(), "RateLoop needs a non-empty level set");
        let idx = Self::rank(&levels, initial_level);
        Self {
            levels,
            idx,
            raise_dwell: raise_dwell.max(1),
            raise_run: 0,
            probe_interval: 0,
            probe_offset: 0,
            frames_since_probe: 0,
        }
    }

    /// Enable active overshoot probing: every `probe_interval` calls to
    /// `level_for_next_transmission` (skipped, per `level_for_next_transmission`'s stall-gating, if
    /// the passive `raise_run` is already making progress), transmit `probe_offset` index-steps
    /// above the current level instead of at it, to get real ground truth where the passive
    /// per-frame recommendation is weak (see
    /// `docs/superpowers/specs/2026-07-25-rateloop-active-overshoot-probing-design.md`).
    /// `probe_interval = 0` (the default from `new`) disables probing entirely.
    pub fn with_probing(mut self, probe_interval: u32, probe_offset: usize) -> Self {
        self.probe_interval = probe_interval;
        self.probe_offset = probe_offset;
        self
    }

    /// The standard coppa level set, starting at the most robust level. `raise_dwell = 5` is the
    /// value found by sweeping 3/4/5/6/8/10/12/15 against `crates/coppa-bench/examples/
    /// closed_loop_arq.rs`'s time-varying-channel bench (`robust` profile): 5 was the peak
    /// (adaptive/best-fixed and adaptive/oracle both highest there and falling off on both
    /// sides), not just a monotonic "more dwell is safer" choice -- see that bench's module doc
    /// for the measured numbers and the deeper reason this configuration still falls short of
    /// the plan's acceptance bar.
    ///
    /// Does NOT enable active overshoot probing (see `with_probing`) -- this constructor is what
    /// `coppa-daemon` uses in production today, which doesn't yet call `level_for_next_transmission`/
    /// `on_probe_result` (that wiring is deliberately deferred, see the probing design doc's
    /// "Out of scope" section), so changing this constructor's defaults would have no effect there
    /// beyond `raise_dwell` itself -- left unchanged from its passive-only-tuned value to avoid an
    /// unintended behavior change to real production traffic. Callers that want probing (currently
    /// only `closed_loop_arq.rs`) opt in explicitly via `.with_probing(2, 1)`, the combination
    /// measured best on that bench.
    pub fn default_coppa() -> Self {
        Self::new(VALID_SPEED_LEVELS.to_vec(), 5, 1)
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

    /// The level to use for the next transmission, and whether it's an active overshoot probe.
    /// Callers should use this in place of `current_level` when probing is enabled (`with_probing`)
    /// and report the outcome via `on_probe_result` (for a probe) or `on_ack`/`on_timeout` (for a
    /// normal transmission) -- never both for the same frame.
    ///
    /// Stall-gated: a probe is skipped (falling back to a normal transmission, without consuming
    /// this interval's turn -- the counter still resets, so the next probe attempt is a full
    /// `probe_interval` away) whenever `raise_run != 0`, i.e. the passive per-frame signal is
    /// already making progress on its own. Probing is for exactly the situation the passive signal
    /// can't make progress in (a channel it reads as persistently low); spending a probe slot while
    /// it's already climbing just trades away guaranteed throughput for no benefit. Measured to
    /// give a small but consistent improvement over unconditional probing on
    /// `crates/coppa-bench/examples/closed_loop_arq.rs` -- see the design doc's "Outcome" section.
    pub fn level_for_next_transmission(&mut self) -> (u8, bool) {
        if self.probe_interval == 0 {
            return (self.current_level(), false);
        }
        self.frames_since_probe += 1;
        if self.frames_since_probe < self.probe_interval {
            return (self.current_level(), false);
        }
        self.frames_since_probe = 0;
        if self.raise_run != 0 {
            return (self.current_level(), false);
        }
        let probe_idx = (self.idx + self.probe_offset).min(self.levels.len() - 1);
        if probe_idx == self.idx {
            // Already at the ceiling -- nothing higher to probe, don't waste this frame on a
            // same-level "probe".
            return (self.current_level(), false);
        }
        (self.levels[probe_idx], true)
    }

    /// Apply the outcome of an active overshoot probe (from `level_for_next_transmission`
    /// returning `true`). Deliberately separate from `on_ack`: a probe is sent above the current
    /// level on purpose, so a failure says nothing about whether the current, lower, untested level
    /// is still safe, and must not drop `idx` or reset `raise_run` the way a real failure does.
    pub fn on_probe_result(&mut self, probed_level: u8, delivered: bool) {
        if delivered {
            // Real, successful decode at this level is stronger ground truth than the passive
            // per-frame estimate raise_dwell is built to filter -- jump straight there.
            self.idx = Self::rank(&self.levels, probed_level);
            self.raise_run = 0;
        }
        // On failure: no-op. idx and raise_run are left exactly as they were.
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

    #[test]
    fn probing_disabled_by_default() {
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 3, 1);
        for _ in 0..20 {
            assert_eq!(r.level_for_next_transmission(), (1, false));
        }
    }

    #[test]
    fn probe_triggers_at_configured_interval_and_offset() {
        // Starting at level 1 (idx 0), offset 2 -> idx 2 -> level 3.
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 3, 1).with_probing(3, 2);
        assert_eq!(r.level_for_next_transmission(), (1, false)); // frame 1
        assert_eq!(r.level_for_next_transmission(), (1, false)); // frame 2
        assert_eq!(r.level_for_next_transmission(), (3, true)); // frame 3 -> probe
        assert_eq!(r.level_for_next_transmission(), (1, false)); // counter reset, frame 1 again
        assert_eq!(r.level_for_next_transmission(), (1, false));
        assert_eq!(r.level_for_next_transmission(), (3, true));
    }

    #[test]
    fn probe_is_stall_gated_and_skipped_while_passively_climbing() {
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 3, 1).with_probing(2, 2);
        r.on_ack(10, true); // raise_run: 0 -> 1, passive signal already making progress
        assert_eq!(r.level_for_next_transmission(), (1, false)); // interval frame 1
                                                                 // Interval elapses here (frame 2), but raise_run != 0 -> gated, falls back to non-probe.
        assert_eq!(r.level_for_next_transmission(), (1, false));
        // Counter reset after the gated attempt; once raise_run is back to 0 the next full
        // interval does probe.
        r.on_ack(1, true); // hold: raise_run -> 0
        assert_eq!(r.level_for_next_transmission(), (1, false)); // interval frame 1
        assert_eq!(r.level_for_next_transmission(), (3, true)); // interval frame 2 -> probes
    }

    #[test]
    fn probe_offset_clamps_at_ceiling_and_falls_back_to_non_probe() {
        // idx already at the last valid level (10); offset would overflow, clamps to idx itself,
        // which is treated as "nothing to probe" -- not a same-level probe.
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 3, 10).with_probing(1, 5);
        assert_eq!(r.level_for_next_transmission(), (10, false));
        assert_eq!(r.level_for_next_transmission(), (10, false));
    }

    #[test]
    fn probe_success_jumps_immediately_bypassing_raise_dwell() {
        // raise_dwell is huge -- would never raise passively within this test.
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 100, 1);
        r.on_probe_result(6, true);
        assert_eq!(r.current_level(), 6);
    }

    #[test]
    fn probe_failure_leaves_idx_and_raise_run_untouched() {
        let mut r = RateLoop::new(VALID_SPEED_LEVELS.to_vec(), 5, 1);
        r.on_ack(10, true); // raise_run: 0 -> 1 (real passive evidence, untouched by the probe below)
        assert_eq!(r.current_level(), 1);
        r.on_probe_result(9, false); // failed probe -- must not touch idx or raise_run
        assert_eq!(r.current_level(), 1);
        r.on_ack(10, true); // raise_run: 1 -> 2, proving it wasn't reset by the failed probe
        r.on_ack(10, true); // raise_run: 2 -> 3
        r.on_ack(10, true); // raise_run: 3 -> 4
        r.on_ack(10, true); // raise_run: 4 -> 5 -> steps up
        assert_eq!(r.current_level(), 2);
    }
}
