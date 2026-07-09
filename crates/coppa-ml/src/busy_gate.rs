//! Spectral-occupancy busy gate (Phase 3 Task 7).
//!
//! Wraps a [`SpectrumSensor`] with the transition-only bookkeeping the daemon's VARA
//! telemetry needs: decision 8 of the Phase 3 system-layer plan calls for a `BUSY
//! ON`/`BUSY OFF` line "from a spectral occupancy gate (`coppa-ml::spectrum_sensor`,
//! threshold = noise floor + 6 dB in the 300-2800 Hz band)". Emitting on every audio
//! block the daemon sees would flood the VARA command port; `BusyGate::observe`
//! mirrors [`crate::cp_gate::CpGate`]'s shape (a small stateful struct fed one block
//! at a time) but returns `Some(new_state)` only on an actual ON/OFF transition, so
//! the caller can emit a `VaraResponse::Busy` exactly when the wire protocol expects
//! one.
//!
//! # Threshold
//!
//! A block is "busy" when [`SpectrumSensor::band_occupancy`] (restricted to Coppa's
//! ~300-2800 Hz SSB passband, margin = 6 dB per decision 8) reports that at least
//! `MIN_OCCUPIED_FRACTION` of the in-band bins exceed `noise_floor() + 6 dB`.
//! `MIN_OCCUPIED_FRACTION` is deliberately a *majority*, not "any bin at all": a
//! single-block (unaveraged) periodogram's per-bin power estimate has real
//! statistical variance even for genuinely quiet input (each bin is roughly
//! exponentially distributed around the true mean), so on any *one* quiet block a
//! sizeable minority of bins will exceed a smoothed floor + 6 dB purely by chance —
//! a low/"any bin" threshold false-triggers on ordinary measurement noise, not a
//! real signal. Requiring a clear majority of in-band bins to be elevated is robust
//! to that per-block variance while still tripping easily on a genuine broadband
//! occupant (which raises essentially all in-band bins at once, not a handful). No
//! sweep against real recordings was run to tune this constant; if it proves
//! twitchy/sluggish in real use, treat it the same way `CpGate`'s doc flags its own
//! untuned constants: adjust with real recordings, not guesswork.
use crate::spectrum_sensor::SpectrumSensor;

/// FFT size used by the internal `SpectrumSensor`. 1024 points gives usable
/// frequency resolution (a few Hz/bin at typical HF audio sample rates) while
/// staying cheap enough to run on every incoming audio block.
const FFT_SIZE: usize = 1024;

/// Lower/upper edges (Hz) of Coppa's SSB passband — see
/// `docs/adr/003-phase1-waveform-break.md` and decision 8's own text.
const BAND_LOW_HZ: f32 = 300.0;
const BAND_HIGH_HZ: f32 = 2800.0;

/// Margin above the noise floor (dB) decision 8 specifies for the occupancy gate.
const MARGIN_DB: f32 = 6.0;

/// Minimum fraction of in-band bins that must exceed the threshold before the
/// gate calls the channel busy (see module doc: a majority, to stay robust to
/// single-block periodogram variance on an otherwise-quiet channel).
const MIN_OCCUPIED_FRACTION: f32 = 0.6;

/// Number of initial blocks spent purely calibrating the noise floor before the
/// gate starts reporting transitions. `SpectrumSensor::noise_floor` defaults to a
/// deep -100 dB sentinel until real blocks have been folded in; gating against that
/// sentinel on the very first block(s) would read almost any real audio as "busy".
/// A handful of blocks is enough for the EWMA (`noise_alpha = 0.1`) to leave the
/// sentinel's immediate vicinity.
const CALIBRATION_BLOCKS: u32 = 3;

/// Spectral-occupancy busy gate. See module doc for the threshold/hysteresis.
pub struct BusyGate {
    sensor: SpectrumSensor,
    busy: bool,
    calibrated_blocks: u32,
}

impl BusyGate {
    /// Create a new gate for audio at `sample_rate` Hz.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sensor: SpectrumSensor::new(FFT_SIZE, sample_rate),
            busy: false,
            calibrated_blocks: 0,
        }
    }

    /// Feed one block of audio samples. Updates the internal noise-floor estimate
    /// and returns `Some(new_state)` only when the busy/not-busy state actually
    /// changes; returns `None` on every other call, including empty input and the
    /// first `CALIBRATION_BLOCKS` calls (see that constant's doc).
    pub fn observe(&mut self, samples: &[f32]) -> Option<bool> {
        if samples.is_empty() {
            return None;
        }

        if self.calibrated_blocks < CALIBRATION_BLOCKS {
            self.sensor.update(samples);
            self.calibrated_blocks += 1;
            return None;
        }

        // Compare this block against the noise floor established by *prior*
        // blocks before folding this one in — gating a block against a statistic
        // partly derived from itself would make ordinary single-block spectral
        // estimation variance (not a real signal) look "occupied".
        let occupancy = self
            .sensor
            .band_occupancy(samples, BAND_LOW_HZ, BAND_HIGH_HZ, MARGIN_DB);
        self.sensor.update(samples);

        let now_busy = occupancy >= MIN_OCCUPIED_FRACTION;
        if now_busy != self.busy {
            self.busy = now_busy;
            Some(now_busy)
        } else {
            None
        }
    }

    /// Current busy state without observing a new block.
    pub fn current(&self) -> bool {
        self.busy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Simple deterministic hash for test "randomness" (mirrors spectrum_sensor's own).
    fn rand_like_hash() -> u32 {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(2654435761)
    }

    fn noise_block(amplitude: f32) -> Vec<f32> {
        (0..1024)
            .map(|_| amplitude * (rand_like_hash() as f32 / u32::MAX as f32 - 0.5))
            .collect()
    }

    fn quiet_block() -> Vec<f32> {
        noise_block(0.01)
    }

    /// A genuinely broadband "noise burst" (unlike a single tone, which only
    /// elevates a narrow sliver of bins near its own frequency): amplitude ~50x
    /// the quiet-block level, so it raises essentially every in-band bin well
    /// above the settled noise floor at once.
    fn band_limited_noise_burst() -> Vec<f32> {
        noise_block(0.5)
    }

    #[test]
    fn starts_not_busy() {
        let gate = BusyGate::new(8000.0);
        assert!(!gate.current());
    }

    #[test]
    fn stays_quiet_on_low_level_noise_only() {
        let mut gate = BusyGate::new(8000.0);
        for _ in 0..20 {
            let transition = gate.observe(&quiet_block());
            assert_eq!(transition, None, "quiet blocks must never trigger BUSY ON");
        }
        assert!(!gate.current());
    }

    #[test]
    fn band_limited_noise_burst_triggers_busy_on_then_off() {
        let mut gate = BusyGate::new(8000.0);

        // Settle the noise floor on quiet blocks first.
        for _ in 0..10 {
            assert_eq!(gate.observe(&quiet_block()), None);
        }
        assert!(!gate.current());

        // Inject a band-limited noise burst -- broadband energy well above the
        // settled floor across the whole 300-2800 Hz band, not a single tone.
        let burst = band_limited_noise_burst();
        let mut saw_busy_on = false;
        for _ in 0..5 {
            if gate.observe(&burst) == Some(true) {
                saw_busy_on = true;
                break;
            }
        }
        assert!(saw_busy_on, "band-limited noise burst must trigger BUSY ON");
        assert!(gate.current());

        // Burst ends; back to quiet -> gate must clear.
        let mut saw_busy_off = false;
        for _ in 0..10 {
            if gate.observe(&quiet_block()) == Some(false) {
                saw_busy_off = true;
                break;
            }
        }
        assert!(saw_busy_off, "channel clearing must trigger BUSY OFF");
        assert!(!gate.current());
    }
}
