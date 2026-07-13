//! Spectrum/waterfall data production for the RX audio stream (Phase 4 Task 4).
//!
//! Turns raw RX audio into the fixed-shape, band-limited, log-magnitude bins
//! `coppa_host::websocket::WsServerMessage::Spectrum` carries to WebSocket
//! clients that have opted in (see that type's doc for the per-client
//! opt-in design). Reuses `coppa_ml::SpectrumSensor`'s existing FFT/power-
//! spectrum plumbing (already used by `BusyGate` in `event_loop.rs`) rather
//! than duplicating FFT setup: `SpectrumSensor::power_spectrum` already
//! returns per-bin power in dB (log-magnitude), so this module's own job is
//! just downsampling ("decimating") that raw per-bin output down to a fixed
//! `SPECTRUM_NUM_BINS`-wide band-limited view, plus the update-rate gate
//! `EventLoop` uses to decide when to actually compute and broadcast one.

use coppa_ml::SpectrumSensor;

/// FFT size for the waterfall's own dedicated `SpectrumSensor` (separate
/// from `BusyGate`'s internal one, which uses a smaller 1024-point FFT tuned
/// for its own occupancy-margin purpose, not bin resolution). At 48 kHz,
/// 4096 points gives ~11.7 Hz/bin raw resolution -- comfortably finer than
/// `SPECTRUM_NUM_BINS` bins spanning the ~2.5 kHz band (~19.5 Hz/bin), so
/// `compute_spectrum_bins` is downsampling (averaging multiple raw bins per
/// output bin), never upsampling emptiness.
pub const SPECTRUM_FFT_SIZE: usize = 4096;

/// Lower edge (Hz) of the waterfall band -- matches Coppa's ~300-2800 Hz SSB
/// passband (see `docs/adr/003-phase1-waveform-break.md`).
pub const SPECTRUM_LOW_HZ: f32 = 300.0;

/// Upper edge (Hz) of the waterfall band.
pub const SPECTRUM_HIGH_HZ: f32 = 2800.0;

/// Number of output bins in a `spectrum` WS message (the brief's "128-bin").
pub const SPECTRUM_NUM_BINS: usize = 128;

/// Target update rate (Hz) for the `spectrum` broadcast (the brief's "4 Hz
/// update") -- gates how often `EventLoop::handle_audio_in` recomputes and
/// broadcasts one, rather than doing so on every audio callback.
pub const SPECTRUM_UPDATE_HZ: f64 = 4.0;

/// Floor value (dB) returned for an output bin with no raw FFT bins mapped
/// into it (degenerate/too-short input) -- matches `SpectrumSensor`'s own
/// default noise-floor starting value, so an empty/silent spectrum reads as
/// "far below anything real" rather than `-inf`/`NaN`.
const NO_SIGNAL_DB: f32 = -100.0;

/// Compute `SPECTRUM_NUM_BINS` log-magnitude (dB) bins spanning
/// `[SPECTRUM_LOW_HZ, SPECTRUM_HIGH_HZ)`, from `samples` (ordinarily
/// `SPECTRUM_FFT_SIZE` samples, though `SpectrumSensor::power_spectrum`
/// itself tolerates a shorter block -- see its own `fft_size.min(samples.len())`
/// sizing -- so this correctly re-derives each call's real bin resolution
/// from the actual returned spectrum length, not `sensor`'s fixed
/// construction-time `fft_size`, the same fix `SpectrumSensor::band_occupancy`
/// already applies for exactly this reason).
///
/// Each output bin averages the LINEAR power of every raw FFT bin whose
/// frequency falls in its sub-range (converting dB -> linear -> dB, not
/// averaging dB values directly, which would understate a narrowband peak
/// straddling a bin boundary) -- this is the "decimated FFT" the task brief
/// asks for: many raw FFT bins collapsed down to a fixed, coarser output
/// resolution.
pub fn compute_spectrum_bins(
    sensor: &SpectrumSensor,
    samples: &[f32],
    sample_rate: f32,
) -> Vec<f32> {
    let spectrum_db = sensor.power_spectrum(samples);
    let n = spectrum_db.len();
    if n < 2 || sample_rate <= 0.0 {
        return vec![NO_SIGNAL_DB; SPECTRUM_NUM_BINS];
    }
    let res = sample_rate / n as f32;
    let half = n / 2; // Only positive frequencies are meaningful.
    let band_width = SPECTRUM_HIGH_HZ - SPECTRUM_LOW_HZ;

    (0..SPECTRUM_NUM_BINS)
        .map(|i| {
            let lo_hz = SPECTRUM_LOW_HZ + band_width * i as f32 / SPECTRUM_NUM_BINS as f32;
            let hi_hz = SPECTRUM_LOW_HZ + band_width * (i + 1) as f32 / SPECTRUM_NUM_BINS as f32;
            let lo_bin = ((lo_hz / res).floor().max(0.0) as usize).min(half.saturating_sub(1));
            let hi_bin = (((hi_hz / res).ceil().max(0.0) as usize).max(lo_bin + 1))
                .min(half.max(lo_bin + 1));

            let raw = &spectrum_db[lo_bin..hi_bin.min(spectrum_db.len())];
            if raw.is_empty() {
                NO_SIGNAL_DB
            } else {
                let linear_sum: f32 = raw.iter().map(|&db| 10f32.powf(db / 10.0)).sum();
                let linear_avg = (linear_sum / raw.len() as f32).max(1e-20);
                10.0 * linear_avg.log10()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_tone(freq: f32, sample_rate: f32, num_samples: usize, amplitude: f32) -> Vec<f32> {
        (0..num_samples)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate).sin())
            .collect()
    }

    #[test]
    fn compute_spectrum_bins_has_the_right_shape() {
        let sensor = SpectrumSensor::new(SPECTRUM_FFT_SIZE, 48_000.0);
        let samples = vec![0.0f32; SPECTRUM_FFT_SIZE];
        let bins = compute_spectrum_bins(&sensor, &samples, 48_000.0);
        assert_eq!(bins.len(), SPECTRUM_NUM_BINS);
    }

    /// Brief's required scenario: "a tone at 1500 Hz peaks in the right bin."
    #[test]
    fn tone_at_1500hz_peaks_in_the_expected_bin() {
        let sample_rate = 48_000.0;
        let sensor = SpectrumSensor::new(SPECTRUM_FFT_SIZE, sample_rate);
        let tone = generate_tone(1500.0, sample_rate, SPECTRUM_FFT_SIZE, 1.0);
        let bins = compute_spectrum_bins(&sensor, &tone, sample_rate);

        let (peak_idx, _) = bins
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .expect("bins must be non-empty");

        // Expected bin: 1500 Hz is (1500 - 300) / ((2800-300)/128) =~ 61.4 bins
        // into the band -> bin index 61.
        let band_width = SPECTRUM_HIGH_HZ - SPECTRUM_LOW_HZ;
        let expected_idx =
            (((1500.0 - SPECTRUM_LOW_HZ) / band_width) * SPECTRUM_NUM_BINS as f32) as usize;

        assert!(
            (peak_idx as i64 - expected_idx as i64).abs() <= 1,
            "1500 Hz tone should peak near bin {expected_idx}, got {peak_idx} \
             (bins: {bins:?})"
        );
    }

    /// A tone outside the 300-2800 Hz band must not show up as the peak bin
    /// of the (band-limited) output -- sanity-checks the band restriction,
    /// not just the binning math.
    #[test]
    fn tone_at_100hz_is_outside_the_band() {
        let sample_rate = 48_000.0;
        let sensor = SpectrumSensor::new(SPECTRUM_FFT_SIZE, sample_rate);
        let in_band_tone = generate_tone(1000.0, sample_rate, SPECTRUM_FFT_SIZE, 1.0);
        let mut mixed = generate_tone(100.0, sample_rate, SPECTRUM_FFT_SIZE, 5.0);
        for (m, t) in mixed.iter_mut().zip(in_band_tone.iter()) {
            *m += t;
        }
        let bins = compute_spectrum_bins(&sensor, &mixed, sample_rate);
        let (peak_idx, _) = bins
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        let band_width = SPECTRUM_HIGH_HZ - SPECTRUM_LOW_HZ;
        let expected_idx =
            (((1000.0 - SPECTRUM_LOW_HZ) / band_width) * SPECTRUM_NUM_BINS as f32) as usize;
        assert!(
            (peak_idx as i64 - expected_idx as i64).abs() <= 1,
            "the stronger out-of-band 100 Hz tone must not dominate the \
             band-limited peak; expected near bin {expected_idx}, got {peak_idx}"
        );
    }

    #[test]
    fn compute_spectrum_bins_handles_shorter_than_fft_size_blocks() {
        // Mirrors SpectrumSensor::band_occupancy's own "fewer samples than
        // fft_size" regression test: must not panic, and must still return
        // the fixed output shape.
        let sample_rate = 48_000.0;
        let sensor = SpectrumSensor::new(SPECTRUM_FFT_SIZE, sample_rate);
        let short = generate_tone(1500.0, sample_rate, SPECTRUM_FFT_SIZE / 2, 1.0);
        let bins = compute_spectrum_bins(&sensor, &short, sample_rate);
        assert_eq!(bins.len(), SPECTRUM_NUM_BINS);
    }
}
