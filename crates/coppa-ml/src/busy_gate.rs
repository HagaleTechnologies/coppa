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
//!
//! # Hold
//!
//! One block's reading is not a state change. A reading that disagrees with the
//! current state must persist, without a single agreeing segment in between, for
//! `HOLD_SECS` (250 ms) of audio before the gate flips; an agreeing segment resets
//! the count. The hold is counted in samples, not calls, so it means the same thing
//! whether the caller feeds 20 ms blocks or one long drain, and it is symmetric: a
//! false `BUSY ON` stalls a client's wait-for-clear (Pat's `waitIfBusy`) as surely
//! as a false `BUSY OFF` lets a transmission out onto an occupied channel, so
//! neither direction is the safe one to rush. Before the hold existed the gate
//! flipped on the first disagreeing block and flapped ON/OFF 3-4 times a second on
//! quiet input (108 transitions in 32 s, H-020 in
//! `docs/reviews/2026-09-05-dim-C-host-apis.md`).
//!
//! [`BusyGate::observe`] splits each call into near-equal segments of at most
//! `FFT_SIZE` samples and measures every one, so a long call is judged across its
//! whole length rather than by its first `FFT_SIZE` samples (the hold credits every
//! sample, so every sample must also be measured). It returns the *net* change
//! across the call. A change is reported when the call returns, even if the flip
//! happened early in it, so the hold for the next change restarts at that report:
//! disagreeing audio in the rest of the reporting call is not counted, which can
//! delay the next change, in [`BusyGate::current`] as well as in the report, by up
//! to that call's length.
//!
//! What a client watching the `BUSY ON`/`BUSY OFF` lines can rely on:
//! - two consecutive transitions are separated by at least 250 ms of received audio;
//! - a state is reported only after it has held for 250 ms, so a real occupant is
//!   reported about 250 ms after it keys up and after it stops (later, by up to one
//!   call's length, when the change follows one reported for a long call);
//! - a busy pulse shorter than 250 ms is never reported.
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

/// How long (seconds of audio) a disagreeing reading must persist before the gate
/// changes state (see module doc's `# Hold`). 250 ms is the value the 2026-09-05
/// H-020 review suggested after its 32 s quiet-input capture; a measurement on
/// synthetic coloured noise showed it removes the flapping, and 500 ms measured the
/// same while doubling detection latency. Like the other constants here it has not
/// been swept against real recordings.
const HOLD_SECS: f32 = 0.25;

/// Spectral-occupancy busy gate. See module doc for the threshold and the hold.
pub struct BusyGate {
    sensor: SpectrumSensor,
    busy: bool,
    calibrated_blocks: u32,
    /// Samples of disagreeing audio needed to flip `busy` (`HOLD_SECS` at the
    /// gate's sample rate).
    hold_samples: usize,
    /// Length, in samples, of the current unbroken run of audio whose reading
    /// disagrees with `busy`, counted from no earlier than the last reported
    /// transition.
    pending_samples: usize,
}

impl BusyGate {
    /// Create a new gate for audio at `sample_rate` Hz.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sensor: SpectrumSensor::new(FFT_SIZE, sample_rate),
            busy: false,
            calibrated_blocks: 0,
            hold_samples: (HOLD_SECS * sample_rate).round() as usize,
            pending_samples: 0,
        }
    }

    /// Feed a block of audio samples of any length. The block is measured in
    /// near-equal segments of at most `FFT_SIZE` samples, each updating the
    /// noise-floor estimate and the hold (see module doc's `# Hold`). Returns
    /// `Some(new_state)` only when the busy/not-busy state differs from what it was
    /// before the call (the net change); returns `None` on every other call,
    /// including empty input, the first `CALIBRATION_BLOCKS` segments (see that
    /// constant's doc), and any call whose disagreeing audio has not yet lasted
    /// `HOLD_SECS` since the last reported transition.
    pub fn observe(&mut self, samples: &[f32]) -> Option<bool> {
        if samples.is_empty() {
            return None;
        }

        let before = self.busy;
        // Equal-length segments rather than `chunks(FFT_SIZE)`: 1030 samples
        // becomes 515 + 515, not 1024 + a 6-sample sliver whose periodogram is
        // meaningless.
        let segments = samples.len().div_ceil(FFT_SIZE);
        for segment in samples.chunks(samples.len().div_ceil(segments)) {
            self.observe_segment(segment);
        }
        if self.busy == before {
            return None;
        }
        // The transition is reported now, at the end of the call, though the flip
        // may have landed early in it. Restart the hold here so the next report is
        // at least `hold_samples` after this one, not after the flip.
        self.pending_samples = 0;
        Some(self.busy)
    }

    /// Measure one segment of at most `FFT_SIZE` samples and advance the hold.
    fn observe_segment(&mut self, samples: &[f32]) {
        if self.calibrated_blocks < CALIBRATION_BLOCKS {
            self.sensor.update(samples);
            self.calibrated_blocks += 1;
            return;
        }

        // Compare this block against the noise floor established by *prior*
        // blocks before folding this one in — gating a block against a statistic
        // partly derived from itself would make ordinary single-block spectral
        // estimation variance (not a real signal) look "occupied".
        let occupancy = self
            .sensor
            .band_occupancy(samples, BAND_LOW_HZ, BAND_HIGH_HZ, MARGIN_DB);
        self.sensor.update(samples);

        let reads_busy = occupancy >= MIN_OCCUPIED_FRACTION;
        if reads_busy == self.busy {
            self.pending_samples = 0;
        } else {
            self.pending_samples += samples.len();
            if self.pending_samples >= self.hold_samples {
                self.busy = reads_busy;
                self.pending_samples = 0;
            }
        }
    }

    /// Current busy state without observing a new block.
    pub fn current(&self) -> bool {
        self.busy
    }

    /// Samples of disagreeing audio needed to change state, `HOLD_SECS` at this
    /// gate's sample rate (for introspection/testing).
    pub fn hold_samples(&self) -> usize {
        self.hold_samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Simple deterministic hash for test "randomness" (mirrors spectrum_sensor's
    // own generator, but threads a per-test-local counter instead of using a
    // shared `static`: a shared static counter here made these tests genuinely
    // flaky under cargo's default parallel test runner, since concurrently
    // running tests stole each other's counter values and corrupted each
    // other's noise sequences -- reproduced by running this module's tests
    // repeatedly under default parallelism (failed most runs) vs.
    // `--test-threads=1` (always passed).
    fn rand_like_hash(counter: &mut u32) -> u32 {
        *counter = counter.wrapping_add(1);
        counter.wrapping_mul(2654435761)
    }

    fn noise_block(amplitude: f32, counter: &mut u32) -> Vec<f32> {
        (0..1024)
            .map(|_| amplitude * (rand_like_hash(counter) as f32 / u32::MAX as f32 - 0.5))
            .collect()
    }

    fn quiet_block(counter: &mut u32) -> Vec<f32> {
        noise_block(0.01, counter)
    }

    /// A genuinely broadband "noise burst" (unlike a single tone, which only
    /// elevates a narrow sliver of bins near its own frequency): amplitude ~50x
    /// the quiet-block level, so it raises essentially every in-band bin well
    /// above the settled noise floor at once.
    fn band_limited_noise_burst(counter: &mut u32) -> Vec<f32> {
        noise_block(0.5, counter)
    }

    #[test]
    fn starts_not_busy() {
        let gate = BusyGate::new(8000.0);
        assert!(!gate.current());
    }

    #[test]
    fn stays_quiet_on_low_level_noise_only() {
        let mut counter = 0u32;
        let mut gate = BusyGate::new(8000.0);
        for _ in 0..20 {
            let transition = gate.observe(&quiet_block(&mut counter));
            assert_eq!(transition, None, "quiet blocks must never trigger BUSY ON");
        }
        assert!(!gate.current());
    }

    #[test]
    fn band_limited_noise_burst_triggers_busy_on_then_off() {
        let mut counter = 0u32;
        let mut gate = BusyGate::new(8000.0);

        // Settle the noise floor on quiet blocks first.
        for _ in 0..10 {
            assert_eq!(gate.observe(&quiet_block(&mut counter)), None);
        }
        assert!(!gate.current());

        // Inject a band-limited noise burst -- broadband energy well above the
        // settled floor across the whole 300-2800 Hz band, not a single tone.
        let burst = band_limited_noise_burst(&mut counter);
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
            if gate.observe(&quiet_block(&mut counter)) == Some(false) {
                saw_busy_off = true;
                break;
            }
        }
        assert!(saw_busy_off, "channel clearing must trigger BUSY OFF");
        assert!(!gate.current());
    }

    /// Quiet noise with a one-pole low-pass tilt (`y = a*y + (1 - a)*x`), a mild
    /// high-frequency rolloff like a sound card's input.
    fn coloured_noise_block(len: usize, a: f32, state: &mut f32, counter: &mut u32) -> Vec<f32> {
        (0..len)
            .map(|_| {
                let x = 0.01 * (rand_like_hash(counter) as f32 / u32::MAX as f32 - 0.5);
                *state = a * *state + (1.0 - a) * x;
                *state
            })
            .collect()
    }

    /// The H-020 field capture: quiet input fed at the daemon's ~20 ms block cadence
    /// (960 samples at 48 kHz) must not produce a single transition in 32 s. With this
    /// module's hash generator `a = 0.3` is the tilt that flaps without the hold (232
    /// transitions in 32 s; 0.1 and 0.2 did not flap, 0.45 and above latched once).
    #[test]
    fn quiet_coloured_noise_does_not_flap_at_daemon_cadence() {
        let mut counter = 0u32;
        let mut state = 0.0f32;
        let mut gate = BusyGate::new(48_000.0);
        let mut transitions = 0;
        for _ in 0..(32 * 48_000 / 960) {
            if gate
                .observe(&coloured_noise_block(960, 0.3, &mut state, &mut counter))
                .is_some()
            {
                transitions += 1;
            }
        }
        assert_eq!(transitions, 0, "quiet input must not flap BUSY ON/OFF");
    }

    /// The ticket's Gherkin: BUSY transitions no more than once per hold interval,
    /// even when the input itself alternates between burst and quiet block to block.
    #[test]
    fn transitions_are_at_least_one_hold_apart() {
        let mut counter = 0u32;
        let mut choice = 0u32;
        let mut gate = BusyGate::new(8000.0);
        let mut position = 0usize;
        let mut transition_positions = Vec::new();
        for _ in 0..400 {
            let block = if rand_like_hash(&mut choice) % 7 < 3 {
                band_limited_noise_burst(&mut counter)
            } else {
                quiet_block(&mut counter)
            };
            position += block.len();
            if gate.observe(&block).is_some() {
                transition_positions.push(position);
            }
        }
        assert!(
            !transition_positions.is_empty(),
            "the burst/quiet mix must produce at least one transition"
        );
        for gap in transition_positions.windows(2) {
            assert!(
                gap[1] - gap[0] >= gate.hold_samples(),
                "transitions at samples {} and {} are closer than the {}-sample hold",
                gap[0],
                gap[1],
                gate.hold_samples()
            );
        }
    }

    #[test]
    fn a_disagreeing_block_restarts_the_hold() {
        let mut counter = 0u32;
        let mut gate = BusyGate::new(8000.0);
        // 2000 samples of hold at 8 kHz: two 1024-sample bursts cover it, one does not.
        assert_eq!(gate.hold_samples(), 2000);
        for _ in 0..10 {
            assert_eq!(gate.observe(&quiet_block(&mut counter)), None);
        }

        assert_eq!(gate.observe(&band_limited_noise_burst(&mut counter)), None);
        assert_eq!(gate.observe(&quiet_block(&mut counter)), None);
        assert_eq!(
            gate.observe(&band_limited_noise_burst(&mut counter)),
            None,
            "the quiet block in between must have restarted the hold"
        );
        assert_eq!(
            gate.observe(&band_limited_noise_burst(&mut counter)),
            Some(true)
        );
        assert!(gate.current());
    }

    /// A busy-wait drain hands the gate thousands of samples in one call; every
    /// sample must be measured, not just the first `FFT_SIZE`.
    #[test]
    fn a_long_call_is_measured_across_its_whole_length() {
        let mut counter = 0u32;
        let mut gate = BusyGate::new(48_000.0);
        for _ in 0..10 {
            assert_eq!(gate.observe(&quiet_block(&mut counter)), None);
        }

        let mut call = quiet_block(&mut counter);
        for _ in 0..13 {
            call.extend(band_limited_noise_burst(&mut counter));
        }
        assert_eq!(gate.observe(&call), Some(true));
        assert!(gate.current());
    }

    /// The daemon drains its whole input ring (up to 8192 samples) in one call after
    /// a stall, so a flip can land early in a long call while the rest of that call
    /// already disagrees with the new state. The next transition must still be
    /// reported at least one hold after this one was reported (when the call
    /// returned), not one hold after the flip inside it.
    #[test]
    fn reported_transitions_are_one_hold_apart_across_a_long_call() {
        let mut counter = 0u32;
        let mut gate = BusyGate::new(48_000.0);
        for _ in 0..10 {
            assert_eq!(gate.observe(&quiet_block(&mut counter)), None);
        }
        // 11 burst blocks: 11264 disagreeing samples, short of the 12000-sample hold.
        for _ in 0..11 {
            assert_eq!(gate.observe(&band_limited_noise_burst(&mut counter)), None);
        }
        // One drain-sized call: the hold completes in its first segment, and the
        // seven quiet segments after it already disagree with BUSY ON.
        let mut call = band_limited_noise_burst(&mut counter);
        for _ in 0..7 {
            call.extend(quiet_block(&mut counter));
        }
        assert_eq!(gate.observe(&call), Some(true));

        let mut since_report = 0;
        let mut reported_off = false;
        for _ in 0..40 {
            let block = quiet_block(&mut counter);
            since_report += block.len();
            if gate.observe(&block) == Some(false) {
                reported_off = true;
                break;
            }
        }
        assert!(reported_off, "the channel clearing must still be reported");
        assert!(
            since_report >= gate.hold_samples(),
            "BUSY OFF reported {since_report} samples after BUSY ON, closer than the {}-sample hold",
            gate.hold_samples()
        );
    }
}
