//! RX/TX audio level metering (COP-65).
//!
//! Gives an operator the numbers needed to set audio levels without a manual
//! calibration ritual: peak dBFS, RMS dBFS and the fraction of clipped samples,
//! measured on raw RX audio every 100 ms and on each whole transmission once.
//! `EventLoop` does the side effects (log lines on RX state transitions, one
//! `TX audio level` line per transmission, and the opt-in WebSocket
//! `audio_level` stream); this module is pure measurement, the RX hysteresis
//! state machine, and the operator-facing wording, so the wording is testable
//! without a log-capture crate.
//!
//! **dBFS convention.** Full scale is sample value 1.0. `rms_dbfs` is
//! `20·log10(sqrt(mean(s²)))` with no AES17 +3 dB correction, so a full-scale
//! sine reads -3.0 dBFS RMS. Silence (and an empty span) reads
//! [`SILENCE_FLOOR_DBFS`], the same floor `CoppaCore::is_squelched` uses.
//!
//! **RX and TX are not symmetric.** The RX meter sees exactly what the sound
//! card delivers, so it can tell an operator that the input is clipping or too
//! quiet. The TX meter sees only the digital waveform Coppa hands to the sound
//! card: every frame is peak-normalized to `TX_PEAK = 0.5` (-6.02 dBFS) in
//! `coppa-codec`'s modulator and no gain is applied afterwards, so real
//! over-drive happens in the OS mixer and the radio's ALC, which the daemon
//! cannot observe. The TX reading is still useful (average power on data frames
//! sits several dB below the tune tone at the same peak), but it never replaces
//! the radio-side ALC check in `docs/OPERATING.md`. A clipped TX sample can only
//! mean a Coppa bug, and is reported as one.
//!
//! **Thresholds are reasoned, not swept.** Like `coppa_ml::CpGate`'s and
//! `coppa_ml::BusyGate`'s constants, every threshold below is hard-coded and has
//! not been checked against real radios. The clip threshold (0.99 of full scale,
//! -0.09 dBFS) comes from `docs/reviews/2026-09-05-dim-A-cli-ux.md` finding 43.
//! The -60 dBFS underdrive threshold sits about 25 dB above a simulated -85 dBFS
//! sound-card noise floor at which decode was still unaffected (COP-65 plan,
//! "Reproduced in this container"); decode degraded only within a few dB of
//! that floor, so -60 dBFS warns well before a drifting chain costs frames.

/// |s| at or above this counts as clipped: 0.99 of full scale, -0.09 dBFS
/// (`docs/reviews/2026-09-05-dim-A-cli-ux.md` finding 43).
pub const CLIP_THRESHOLD: f32 = 0.99;

/// Reported for digital silence; matches `CoppaCore::is_squelched`'s floor.
pub const SILENCE_FLOOR_DBFS: f32 = -120.0;

/// RX meter windows per second (`docs/reviews/2026-09-05-dim-H-missing-surfaces.md`
/// finding 6: "~10 Hz").
pub const LEVEL_UPDATE_HZ: u32 = 10;

/// Consecutive clipped windows before `Clipping` (2 = 200 ms: rejects a single
/// T/R-switch pop, still trips on real over-drive, which clips every window).
pub const CLIP_ENTER_WINDOWS: u32 = 2;

/// Consecutive clean windows before leaving `Clipping` (3 s, so the warning
/// does not flap during gain adjustment).
pub const CLIP_CLEAR_WINDOWS: u32 = 30;

/// Window RMS below which input counts as low. Unswept: ~25 dB above a
/// simulated -85 dBFS sound-card floor where decode was still unaffected.
pub const UNDERDRIVE_RMS_DBFS: f32 = -60.0;

/// Recovery threshold (3 dB of hysteresis above [`UNDERDRIVE_RMS_DBFS`]).
pub const UNDERDRIVE_CLEAR_RMS_DBFS: f32 = -57.0;

/// Consecutive low windows before `Underdrive` (5 s: a pause between
/// transmissions on a quiet band must not warn).
pub const UNDERDRIVE_ENTER_WINDOWS: u32 = 50;

/// Consecutive windows at or above [`UNDERDRIVE_CLEAR_RMS_DBFS`] before leaving
/// `Underdrive` (1 s).
pub const UNDERDRIVE_CLEAR_WINDOWS: u32 = 10;

/// Operator wording for the TX self-check: every frame is normalized to
/// -6.02 dBFS, so a clipped TX sample is a defect, not a level setting.
pub const TX_CLIPPING_MESSAGE: &str =
    "TX audio is clipping before it leaves Coppa: this is a Coppa bug, not a drive-level setting; please report it";

const RX_CLIPPING_MESSAGE: &str =
    "RX audio input is clipping: turn down the radio's audio output or the sound card's input gain";
const RX_TOO_LOW_MESSAGE: &str =
    "RX audio input level is too low: turn up the radio's audio output or the sound card's input gain";
const RX_SILENT_MESSAGE: &str =
    "RX audio input is silent: check that the sound card input is connected and not muted";
const RX_OK_MESSAGE: &str = "RX audio input level OK";

/// Level measurement over one span of audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelReading {
    /// `20·log10(max |s|)`, floored at [`SILENCE_FLOOR_DBFS`].
    pub peak_dbfs: f32,
    /// `20·log10(sqrt(mean(s²)))`, floored at [`SILENCE_FLOOR_DBFS`].
    pub rms_dbfs: f32,
    /// Fraction (0.0-1.0) of samples with `|s| >= CLIP_THRESHOLD`; non-finite
    /// samples count as clipped.
    pub clip_fraction: f32,
    /// Number of samples the reading covers.
    pub samples: usize,
}

/// Level state, for RX (all three) and TX (`Ok` or `Clipping`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelState {
    Ok,
    Clipping,
    Underdrive,
}

/// One completed RX window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RxLevelUpdate {
    /// The window's measurement.
    pub reading: LevelReading,
    /// The meter's hysteresis state after this window.
    pub state: LevelState,
    /// `Some(new_state)` only when this window changed the state.
    pub transition: Option<LevelState>,
}

/// Running totals shared by [`measure`] and [`RxLevelMeter`], so both use one
/// definition of peak, RMS and clipping.
#[derive(Debug, Default)]
struct Accumulator {
    peak: f32,
    sum_sq: f64,
    clipped: usize,
    count: usize,
}

impl Accumulator {
    /// Fold one sample in. A non-finite sample counts as magnitude 1.0, so it
    /// reads as clipped and can never poison the totals with NaN/inf.
    fn push(&mut self, sample: f32) {
        let mag = if sample.is_finite() {
            sample.abs()
        } else {
            1.0
        };
        if mag > self.peak {
            self.peak = mag;
        }
        let mag = f64::from(mag);
        self.sum_sq += mag * mag;
        if mag >= f64::from(CLIP_THRESHOLD) {
            self.clipped += 1;
        }
        self.count += 1;
    }

    fn reading(&self) -> LevelReading {
        if self.count == 0 {
            return LevelReading {
                peak_dbfs: SILENCE_FLOOR_DBFS,
                rms_dbfs: SILENCE_FLOOR_DBFS,
                clip_fraction: 0.0,
                samples: 0,
            };
        }
        let n = self.count as f64;
        LevelReading {
            peak_dbfs: to_dbfs(f64::from(self.peak)),
            rms_dbfs: to_dbfs((self.sum_sq / n).sqrt()),
            clip_fraction: (self.clipped as f64 / n) as f32,
            samples: self.count,
        }
    }
}

/// Linear amplitude to dBFS, floored at [`SILENCE_FLOOR_DBFS`].
fn to_dbfs(amplitude: f64) -> f32 {
    if amplitude > 0.0 {
        ((20.0 * amplitude.log10()) as f32).max(SILENCE_FLOOR_DBFS)
    } else {
        SILENCE_FLOOR_DBFS
    }
}

/// Measure one span of audio (a whole transmission, or a test fixture).
pub fn measure(samples: &[f32]) -> LevelReading {
    let mut acc = Accumulator::default();
    for &s in samples {
        acc.push(s);
    }
    acc.reading()
}

/// RX meter: fed arbitrary chunks of raw received audio, it emits one
/// [`RxLevelUpdate`] per completed 100 ms window and runs the
/// `Ok`/`Clipping`/`Underdrive` hysteresis.
///
/// Cadence is by sample count, not wall clock, so a window always covers
/// exactly `sample_rate / LEVEL_UPDATE_HZ` samples whatever the caller's chunk
/// size.
pub struct RxLevelMeter {
    window_len: usize,
    acc: Accumulator,
    state: LevelState,
    /// Consecutive windows containing a clipped sample.
    clip_run: u32,
    /// Consecutive windows with no clipped sample.
    clean_run: u32,
    /// Consecutive windows with RMS below `UNDERDRIVE_RMS_DBFS`.
    low_run: u32,
    /// Consecutive windows with RMS at or above `UNDERDRIVE_CLEAR_RMS_DBFS`.
    recovered_run: u32,
}

impl RxLevelMeter {
    /// A meter in the `Ok` state with 100 ms windows at `sample_rate`
    /// (clamped to at least one sample per window).
    pub fn new(sample_rate: u32) -> Self {
        Self {
            window_len: (sample_rate / LEVEL_UPDATE_HZ).max(1) as usize,
            acc: Accumulator::default(),
            state: LevelState::Ok,
            clip_run: 0,
            clean_run: 0,
            low_run: 0,
            recovered_run: 0,
        }
    }

    /// Feed `samples`; returns one update per window completed by this chunk
    /// (none if the chunk only extends the in-progress window).
    pub fn observe(&mut self, samples: &[f32]) -> Vec<RxLevelUpdate> {
        let mut updates = Vec::new();
        for &s in samples {
            self.acc.push(s);
            if self.acc.count >= self.window_len {
                let reading = self.acc.reading();
                self.acc = Accumulator::default();
                let transition = self.step(&reading);
                updates.push(RxLevelUpdate {
                    reading,
                    state: self.state,
                    transition,
                });
            }
        }
        updates
    }

    /// Drop the in-progress window (called while transmitting, so audio from
    /// before and after a transmission is never merged into one window). The
    /// state and the consecutive-window runs persist.
    pub fn discard_partial_window(&mut self) {
        self.acc = Accumulator::default();
    }

    /// Current hysteresis state.
    #[cfg(test)]
    pub fn state(&self) -> LevelState {
        self.state
    }

    /// Advance the state machine by one completed window; returns the new
    /// state only if it changed.
    fn step(&mut self, reading: &LevelReading) -> Option<LevelState> {
        if reading.clip_fraction > 0.0 {
            self.clip_run = self.clip_run.saturating_add(1);
            self.clean_run = 0;
        } else {
            self.clean_run = self.clean_run.saturating_add(1);
            self.clip_run = 0;
        }

        let previous = self.state;
        if self.state == LevelState::Clipping {
            if self.clean_run >= CLIP_CLEAR_WINDOWS {
                self.state = LevelState::Ok;
                self.low_run = 0;
                self.recovered_run = 0;
            }
        } else if self.clip_run >= CLIP_ENTER_WINDOWS {
            // Clipping overrides underdrive: an input that is both quiet on
            // average and clipping is clipping.
            self.state = LevelState::Clipping;
        } else {
            if reading.rms_dbfs < UNDERDRIVE_RMS_DBFS {
                self.low_run = self.low_run.saturating_add(1);
            } else {
                self.low_run = 0;
            }
            if reading.rms_dbfs >= UNDERDRIVE_CLEAR_RMS_DBFS {
                self.recovered_run = self.recovered_run.saturating_add(1);
            } else {
                self.recovered_run = 0;
            }
            match self.state {
                LevelState::Ok if self.low_run >= UNDERDRIVE_ENTER_WINDOWS => {
                    self.state = LevelState::Underdrive;
                }
                LevelState::Underdrive if self.recovered_run >= UNDERDRIVE_CLEAR_WINDOWS => {
                    self.state = LevelState::Ok;
                }
                _ => {}
            }
        }
        (self.state != previous).then_some(self.state)
    }
}

/// TX classification: `Clipping` iff any sample reached [`CLIP_THRESHOLD`].
/// Never `Underdrive`: Coppa's own TX level is fixed, and the real drive level
/// is set after the sound card, where the daemon cannot see it.
pub fn tx_state(reading: &LevelReading) -> LevelState {
    if reading.clip_fraction > 0.0 {
        LevelState::Clipping
    } else {
        LevelState::Ok
    }
}

/// Span covered by a reading, in ms (`samples * 1000 / sample_rate`, 0 if the
/// rate is 0). Used by both the `TX audio level` log line and the WebSocket
/// message.
pub fn duration_ms(reading: &LevelReading, sample_rate: u32) -> u32 {
    if sample_rate == 0 {
        return 0;
    }
    let ms = reading.samples as u64 * 1000 / u64::from(sample_rate);
    u32::try_from(ms).unwrap_or(u32::MAX)
}

/// Round to 0.1 dB; used for both log fields and wire fields so they always
/// agree.
pub fn round_db(db: f32) -> f32 {
    (db * 10.0).round() / 10.0
}

/// Operator wording for an RX transition into `state`. `reading` is the window
/// that caused it: an underdrive whose RMS sits on the silence floor gets the
/// "silent" wording instead of "too low".
pub fn rx_transition_message(state: LevelState, reading: &LevelReading) -> &'static str {
    match state {
        LevelState::Clipping => RX_CLIPPING_MESSAGE,
        LevelState::Underdrive if reading.rms_dbfs <= SILENCE_FLOOR_DBFS => RX_SILENT_MESSAGE,
        LevelState::Underdrive => RX_TOO_LOW_MESSAGE,
        LevelState::Ok => RX_OK_MESSAGE,
    }
}

/// Build the WebSocket `audio_level` message for one reading. dB fields go
/// through [`round_db`], like the log lines.
#[cfg(feature = "websocket")]
pub fn ws_message(
    direction: coppa_host::websocket::AudioLevelDirection,
    reading: &LevelReading,
    state: LevelState,
    sample_rate: u32,
) -> coppa_host::websocket::WsServerMessage {
    use coppa_host::websocket::{AudioLevelState, WsServerMessage};

    let state = match state {
        LevelState::Ok => AudioLevelState::Ok,
        LevelState::Clipping => AudioLevelState::Clipping,
        LevelState::Underdrive => AudioLevelState::Underdrive,
    };
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    WsServerMessage::AudioLevel {
        direction,
        peak_dbfs: round_db(reading.peak_dbfs),
        rms_dbfs: round_db(reading.rms_dbfs),
        clip_fraction: reading.clip_fraction,
        state,
        duration_ms: duration_ms(reading, sample_rate),
        timestamp_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 100 ms window (48 kHz) of a constant level.
    fn window(level: f32) -> Vec<f32> {
        vec![level; 4800]
    }

    /// One 100 ms window (48 kHz) with a single full-scale sample, rest 0.01.
    fn clipped_window() -> Vec<f32> {
        let mut w = window(0.01);
        w[100] = 1.0;
        w
    }

    /// Feed exactly one window and return its update.
    fn feed(meter: &mut RxLevelMeter, samples: &[f32]) -> RxLevelUpdate {
        let updates = meter.observe(samples);
        assert_eq!(updates.len(), 1, "expected exactly one window");
        updates[0]
    }

    #[test]
    fn measure_half_scale_sine_reads_minus_6_peak_and_minus_9_rms() {
        let samples: Vec<f32> = (0..4800)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let r = measure(&samples);
        assert!((r.peak_dbfs - (-6.02)).abs() < 0.05, "peak {}", r.peak_dbfs);
        assert!((r.rms_dbfs - (-9.03)).abs() < 0.05, "rms {}", r.rms_dbfs);
        assert_eq!(r.clip_fraction, 0.0);
        assert_eq!(r.samples, 4800);
    }

    #[test]
    fn measure_silence_and_empty_input_read_the_floor() {
        let r = measure(&[0.0; 100]);
        assert_eq!(r.peak_dbfs, SILENCE_FLOOR_DBFS);
        assert_eq!(r.rms_dbfs, SILENCE_FLOOR_DBFS);
        assert_eq!(r.clip_fraction, 0.0);
        assert_eq!(r.samples, 100);

        let r = measure(&[]);
        assert_eq!(r.peak_dbfs, SILENCE_FLOOR_DBFS);
        assert_eq!(r.rms_dbfs, SILENCE_FLOOR_DBFS);
        assert_eq!(r.clip_fraction, 0.0);
        assert_eq!(r.samples, 0);
    }

    #[test]
    fn measure_counts_samples_at_or_above_0_99_as_clipped() {
        let r = measure(&[0.98, 0.99, -1.0, 0.5]);
        assert_eq!(r.clip_fraction, 0.5);
    }

    #[test]
    fn measure_treats_non_finite_samples_as_clipped_full_scale() {
        let r = measure(&[f32::NAN, f32::INFINITY, 0.0, 0.0]);
        assert!(r.peak_dbfs.is_finite());
        assert!(r.rms_dbfs.is_finite());
        assert!(r.clip_fraction.is_finite());
        assert_eq!(r.peak_dbfs, 0.0);
        assert_eq!(r.clip_fraction, 0.5);
    }

    #[test]
    fn rx_meter_emits_one_update_per_100ms_window_across_arbitrary_chunking() {
        let mut meter = RxLevelMeter::new(48_000);
        let mut all = Vec::new();

        let first = meter.observe(&vec![0.01; 4799]);
        assert!(first.is_empty());
        let second = meter.observe(&[0.01]);
        assert_eq!(second.len(), 1);
        all.extend(second);

        let mut later = Vec::new();
        for _ in 0..15 {
            later.extend(meter.observe(&vec![0.01; 640]));
        }
        assert_eq!(later.len(), 2);
        all.extend(later);

        for update in all {
            assert_eq!(update.reading.samples, 4800);
        }
    }

    #[test]
    fn rx_meter_window_length_follows_sample_rate() {
        let mut meter = RxLevelMeter::new(8_000);
        assert_eq!(meter.observe(&vec![0.01; 800]).len(), 1);

        let mut zero_rate = RxLevelMeter::new(0);
        assert_eq!(zero_rate.observe(&[0.01, 0.01]).len(), 2);
    }

    #[test]
    fn a_single_clipped_window_does_not_enter_clipping() {
        let mut meter = RxLevelMeter::new(48_000);
        let u = feed(&mut meter, &clipped_window());
        assert!(u.reading.clip_fraction > 0.0);
        assert_eq!(u.state, LevelState::Ok);
        assert_eq!(u.transition, None);
    }

    #[test]
    fn two_consecutive_clipped_windows_enter_clipping() {
        let mut meter = RxLevelMeter::new(48_000);
        feed(&mut meter, &clipped_window());
        let u = feed(&mut meter, &clipped_window());
        assert_eq!(u.state, LevelState::Clipping);
        assert_eq!(u.transition, Some(LevelState::Clipping));
    }

    #[test]
    fn clipping_clears_after_30_consecutive_clean_windows() {
        let mut meter = RxLevelMeter::new(48_000);
        feed(&mut meter, &clipped_window());
        feed(&mut meter, &clipped_window());
        assert_eq!(meter.state(), LevelState::Clipping);

        // A clipped window at clean-run 20 restarts the count.
        for _ in 0..20 {
            let u = feed(&mut meter, &window(0.01));
            assert_eq!(u.state, LevelState::Clipping);
            assert_eq!(u.transition, None);
        }
        let u = feed(&mut meter, &clipped_window());
        assert_eq!(u.state, LevelState::Clipping);
        assert_eq!(u.transition, None);

        for _ in 0..29 {
            let u = feed(&mut meter, &window(0.01));
            assert_eq!(u.state, LevelState::Clipping);
            assert_eq!(u.transition, None);
        }
        let u = feed(&mut meter, &window(0.01));
        assert_eq!(u.state, LevelState::Ok);
        assert_eq!(u.transition, Some(LevelState::Ok));
    }

    #[test]
    fn underdrive_needs_50_consecutive_windows_below_minus_60() {
        let mut meter = RxLevelMeter::new(48_000);
        for _ in 0..49 {
            let u = feed(&mut meter, &window(0.0007));
            assert!(u.reading.rms_dbfs < UNDERDRIVE_RMS_DBFS);
            assert_eq!(u.state, LevelState::Ok);
            assert_eq!(u.transition, None);
        }
        // One normal window resets the run.
        let u = feed(&mut meter, &window(0.01));
        assert_eq!(u.state, LevelState::Ok);

        for _ in 0..49 {
            let u = feed(&mut meter, &window(0.0007));
            assert_eq!(u.state, LevelState::Ok);
            assert_eq!(u.transition, None);
        }
        let u = feed(&mut meter, &window(0.0007));
        assert_eq!(u.state, LevelState::Underdrive);
        assert_eq!(u.transition, Some(LevelState::Underdrive));
    }

    #[test]
    fn underdrive_clears_only_after_10_windows_at_or_above_minus_57() {
        let mut meter = RxLevelMeter::new(48_000);
        for _ in 0..UNDERDRIVE_ENTER_WINDOWS {
            feed(&mut meter, &window(0.0007));
        }
        assert_eq!(meter.state(), LevelState::Underdrive);

        // Inside the hysteresis band: above -60 but below -57 never clears.
        for _ in 0..100 {
            let u = feed(&mut meter, &window(0.0012));
            assert!(u.reading.rms_dbfs > UNDERDRIVE_RMS_DBFS);
            assert!(u.reading.rms_dbfs < UNDERDRIVE_CLEAR_RMS_DBFS);
            assert_eq!(u.state, LevelState::Underdrive);
            assert_eq!(u.transition, None);
        }

        for _ in 0..9 {
            let u = feed(&mut meter, &window(0.01));
            assert_eq!(u.state, LevelState::Underdrive);
            assert_eq!(u.transition, None);
        }
        let u = feed(&mut meter, &window(0.01));
        assert_eq!(u.state, LevelState::Ok);
        assert_eq!(u.transition, Some(LevelState::Ok));
    }

    #[test]
    fn clipping_overrides_underdrive() {
        let mut meter = RxLevelMeter::new(48_000);
        for _ in 0..UNDERDRIVE_ENTER_WINDOWS {
            feed(&mut meter, &window(0.0007));
        }
        assert_eq!(meter.state(), LevelState::Underdrive);

        let u = feed(&mut meter, &clipped_window());
        assert_eq!(u.transition, None);
        let u = feed(&mut meter, &clipped_window());
        assert_eq!(u.state, LevelState::Clipping);
        assert_eq!(u.transition, Some(LevelState::Clipping));
    }

    #[test]
    fn discard_partial_window_drops_accumulated_samples_but_keeps_state() {
        let mut meter = RxLevelMeter::new(48_000);
        feed(&mut meter, &clipped_window());
        feed(&mut meter, &clipped_window());
        assert_eq!(meter.state(), LevelState::Clipping);

        assert!(meter.observe(&vec![1.0; 2400]).is_empty());
        meter.discard_partial_window();
        let updates = meter.observe(&window(0.01));
        assert_eq!(updates.len(), 1);
        let u = updates[0];
        assert_eq!(u.reading.clip_fraction, 0.0);
        assert!((u.reading.peak_dbfs - (-40.0)).abs() < 0.1);
        assert_eq!(u.state, LevelState::Clipping, "state survives a discard");
    }

    #[test]
    fn tx_state_is_clipping_only_when_a_sample_reaches_0_99() {
        assert_eq!(tx_state(&measure(&[0.5; 100])), LevelState::Ok);
        assert_eq!(tx_state(&measure(&[0.5, 0.99])), LevelState::Clipping);
        assert_eq!(tx_state(&measure(&[0.0; 100])), LevelState::Ok);
        assert_eq!(tx_state(&measure(&[])), LevelState::Ok);
    }

    #[test]
    fn duration_and_rounding_helpers() {
        let tx = LevelReading {
            peak_dbfs: -6.0,
            rms_dbfs: -16.8,
            clip_fraction: 0.0,
            samples: 65_520,
        };
        let rx = LevelReading {
            samples: 4800,
            ..tx
        };
        assert_eq!(duration_ms(&tx, 48_000), 1365);
        assert_eq!(duration_ms(&rx, 48_000), 100);
        assert_eq!(duration_ms(&tx, 0), 0);
        assert_eq!(round_db(-16.7801), -16.8);
        assert_eq!(round_db(-6.0206), -6.0);
    }

    #[test]
    fn transition_messages_name_the_fix() {
        let base = LevelReading {
            peak_dbfs: -30.0,
            rms_dbfs: -62.7,
            clip_fraction: 0.0,
            samples: 4800,
        };
        let clipping = rx_transition_message(LevelState::Clipping, &base);
        assert!(clipping.contains("clipping") && clipping.contains("turn down"));

        let low = rx_transition_message(LevelState::Underdrive, &base);
        assert!(low.contains("too low") && low.contains("turn up"));

        let silent_reading = LevelReading {
            rms_dbfs: SILENCE_FLOOR_DBFS,
            ..base
        };
        let silent = rx_transition_message(LevelState::Underdrive, &silent_reading);
        assert!(silent.contains("silent") && silent.contains("muted"));

        assert!(rx_transition_message(LevelState::Ok, &base).contains("level OK"));
        assert!(TX_CLIPPING_MESSAGE.contains("Coppa bug"));
    }

    #[cfg(feature = "websocket")]
    #[test]
    fn ws_message_rounds_to_a_tenth_of_a_db_and_reports_duration() {
        use coppa_host::websocket::{AudioLevelDirection, AudioLevelState, WsServerMessage};

        let reading = LevelReading {
            peak_dbfs: -6.0206,
            rms_dbfs: -16.7801,
            clip_fraction: 0.0,
            samples: 65_520,
        };
        match ws_message(AudioLevelDirection::Tx, &reading, LevelState::Ok, 48_000) {
            WsServerMessage::AudioLevel {
                direction,
                peak_dbfs,
                rms_dbfs,
                clip_fraction,
                state,
                duration_ms,
                timestamp_ms,
            } => {
                assert_eq!(direction, AudioLevelDirection::Tx);
                assert_eq!(peak_dbfs, -6.0);
                assert_eq!(rms_dbfs, -16.8);
                assert_eq!(clip_fraction, 0.0);
                assert_eq!(state, AudioLevelState::Ok);
                assert_eq!(duration_ms, 1365);
                assert!(timestamp_ms > 0);
            }
            other => panic!("expected AudioLevel, got {:?}", other),
        }
    }
}
