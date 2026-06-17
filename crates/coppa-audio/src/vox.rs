//! VOX (Voice Operated Switch) detector.
//!
//! Detects audio activity using energy thresholding with configurable
//! hang time and debounce to avoid rapid toggling.

/// VOX detector state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxState {
    /// No audio activity detected.
    Idle,
    /// Audio activity is active.
    Active,
    /// Audio dropped below threshold but hang timer is running.
    Hanging,
    /// Debouncing a transition.
    Debouncing,
}

/// VOX detector with energy threshold, hang time, and debounce.
pub struct VoxDetector {
    /// Energy threshold (RMS squared) for triggering.
    threshold: f32,
    /// Hang time in samples before transitioning from Active to Idle.
    hang_samples: usize,
    /// Debounce time in samples before confirming a transition.
    debounce_samples: usize,
    /// Current state.
    state: VoxState,
    /// Counter for hang timer.
    hang_counter: usize,
    /// Counter for debounce timer.
    debounce_counter: usize,
    /// Whether the last debounce was toward active.
    debounce_toward_active: bool,
}

impl VoxDetector {
    /// Create a new VOX detector.
    ///
    /// * `threshold_db` - Energy threshold in dBFS (e.g., -30.0)
    /// * `hang_time_ms` - Hang time in milliseconds
    /// * `debounce_ms` - Debounce time in milliseconds
    /// * `sample_rate` - Audio sample rate in Hz
    pub fn new(threshold_db: f32, hang_time_ms: f32, debounce_ms: f32, sample_rate: u32) -> Self {
        let threshold = 10.0f32.powf(threshold_db / 10.0);
        let hang_samples = (hang_time_ms * sample_rate as f32 / 1000.0) as usize;
        let debounce_samples = (debounce_ms * sample_rate as f32 / 1000.0) as usize;
        Self {
            threshold,
            hang_samples,
            debounce_samples,
            state: VoxState::Idle,
            hang_counter: 0,
            debounce_counter: 0,
            debounce_toward_active: false,
        }
    }

    /// Process a block of samples and return whether the VOX is active.
    pub fn process(&mut self, samples: &[f32]) -> bool {
        let energy = if samples.is_empty() {
            0.0
        } else {
            samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32
        };

        let above_threshold = energy > self.threshold;

        match self.state {
            VoxState::Idle => {
                if above_threshold {
                    if self.debounce_samples == 0 {
                        self.state = VoxState::Active;
                    } else {
                        self.state = VoxState::Debouncing;
                        self.debounce_counter = self.debounce_samples;
                        self.debounce_toward_active = true;
                    }
                }
            }
            VoxState::Active => {
                if !above_threshold {
                    self.state = VoxState::Hanging;
                    self.hang_counter = self.hang_samples;
                }
            }
            VoxState::Hanging => {
                if above_threshold {
                    self.state = VoxState::Active;
                } else if self.hang_counter == 0 {
                    self.state = VoxState::Idle;
                } else {
                    self.hang_counter = self.hang_counter.saturating_sub(samples.len());
                }
            }
            VoxState::Debouncing => {
                if self.debounce_counter == 0 {
                    if self.debounce_toward_active && above_threshold {
                        self.state = VoxState::Active;
                    } else {
                        self.state = VoxState::Idle;
                    }
                } else {
                    self.debounce_counter = self.debounce_counter.saturating_sub(samples.len());
                }
            }
        }

        self.is_active()
    }

    /// Returns whether the VOX is currently active (transmitting).
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            VoxState::Active | VoxState::Hanging | VoxState::Debouncing
        )
    }

    /// Current state of the VOX detector.
    pub fn state(&self) -> VoxState {
        self.state
    }

    /// Reset the VOX detector to idle.
    pub fn reset(&mut self) {
        self.state = VoxState::Idle;
        self.hang_counter = 0;
        self.debounce_counter = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vox_silence() {
        let mut vox = VoxDetector::new(-30.0, 500.0, 0.0, 48000);
        let silence = vec![0.0f32; 480];
        assert!(!vox.process(&silence));
        assert_eq!(vox.state(), VoxState::Idle);
    }

    #[test]
    fn test_vox_tone_activates() {
        let mut vox = VoxDetector::new(-30.0, 500.0, 0.0, 48000);
        // A loud tone
        let tone: Vec<f32> = (0..480)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin())
            .collect();
        assert!(vox.process(&tone));
        assert_eq!(vox.state(), VoxState::Active);
    }

    #[test]
    fn test_vox_hang_time() {
        let mut vox = VoxDetector::new(-30.0, 100.0, 0.0, 48000);
        // Activate with tone
        let tone: Vec<f32> = (0..480)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin())
            .collect();
        vox.process(&tone);
        assert!(vox.is_active());

        // Feed silence - should hang
        let silence = vec![0.0f32; 480];
        let active = vox.process(&silence);
        assert!(active); // Still active during hang time
        assert_eq!(vox.state(), VoxState::Hanging);
    }

    #[test]
    fn test_vox_hang_expires() {
        let mut vox = VoxDetector::new(-30.0, 10.0, 0.0, 48000);
        let tone: Vec<f32> = (0..480)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin())
            .collect();
        vox.process(&tone);

        // Feed enough silence to expire hang
        let silence = vec![0.0f32; 4800];
        for _ in 0..10 {
            vox.process(&silence);
        }
        assert!(!vox.is_active());
        assert_eq!(vox.state(), VoxState::Idle);
    }

    #[test]
    fn test_vox_reset() {
        let mut vox = VoxDetector::new(-30.0, 500.0, 0.0, 48000);
        let tone: Vec<f32> = (0..480)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin())
            .collect();
        vox.process(&tone);
        assert!(vox.is_active());
        vox.reset();
        assert_eq!(vox.state(), VoxState::Idle);
        assert!(!vox.is_active());
    }

    #[test]
    fn test_vox_debounce() {
        let mut vox = VoxDetector::new(-30.0, 500.0, 50.0, 48000);
        let tone: Vec<f32> = (0..480)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin())
            .collect();
        // First detection enters debouncing
        vox.process(&tone);
        assert_eq!(vox.state(), VoxState::Debouncing);
    }
}
