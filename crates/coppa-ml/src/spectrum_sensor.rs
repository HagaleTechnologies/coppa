//! FFT-based spectrum sensing and noise floor estimation.

use num_complex::Complex32;
use rustfft::FftPlanner;

/// FFT-based spectrum analyzer for channel sensing.
pub struct SpectrumSensor {
    fft_size: usize,
    sample_rate: f32,
    noise_floor: f32,
    /// EWMA smoothing factor for noise floor.
    noise_alpha: f32,
    /// Power spectrum accumulator for averaging.
    spectrum_accum: Vec<f32>,
    accum_count: usize,
}

impl SpectrumSensor {
    /// Create a new spectrum sensor.
    ///
    /// * `fft_size` - FFT size (must be power of 2)
    /// * `sample_rate` - Sample rate in Hz
    pub fn new(fft_size: usize, sample_rate: f32) -> Self {
        Self {
            fft_size,
            sample_rate,
            noise_floor: -100.0,
            noise_alpha: 0.1,
            spectrum_accum: vec![0.0; fft_size],
            accum_count: 0,
        }
    }

    /// Compute the power spectrum of a block of samples.
    ///
    /// Returns power in dB for each frequency bin.
    pub fn power_spectrum(&self, samples: &[f32]) -> Vec<f32> {
        let n = self.fft_size.min(samples.len());
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(n);

        // Apply Hann window and convert to complex
        let mut buffer: Vec<Complex32> = samples[..n]
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let window = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos());
                Complex32::new(s * window, 0.0)
            })
            .collect();

        fft.process(&mut buffer);

        // Convert to power in dB
        buffer
            .iter()
            .map(|c| {
                let power = (c.norm_sqr() / n as f32).max(1e-20);
                10.0 * power.log10()
            })
            .collect()
    }

    /// Process a block and update noise floor estimate.
    pub fn update(&mut self, samples: &[f32]) {
        let spectrum = self.power_spectrum(samples);

        // Accumulate for averaging
        for (acc, &s) in self.spectrum_accum.iter_mut().zip(spectrum.iter()) {
            *acc += s;
        }
        self.accum_count += 1;

        // Estimate noise floor as the median of the lower half of the spectrum
        let mut sorted: Vec<f32> = spectrum.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[sorted.len() / 4]; // 25th percentile ≈ noise floor

        // EWMA update
        if self.accum_count == 1 {
            self.noise_floor = median;
        } else {
            self.noise_floor =
                self.noise_alpha * median + (1.0 - self.noise_alpha) * self.noise_floor;
        }
    }

    /// Current noise floor estimate in dB.
    pub fn noise_floor(&self) -> f32 {
        self.noise_floor
    }

    /// Detect if a tone is present at the given frequency.
    ///
    /// Returns true if the power at that frequency exceeds the noise floor
    /// by at least `threshold_db`.
    pub fn detect_tone(&self, samples: &[f32], freq_hz: f32, threshold_db: f32) -> bool {
        let spectrum = self.power_spectrum(samples);
        let bin = (freq_hz * self.fft_size as f32 / self.sample_rate) as usize;

        if bin >= spectrum.len() {
            return false;
        }

        // Check if the bin power exceeds noise floor by threshold
        spectrum[bin] - self.noise_floor > threshold_db
    }

    /// Estimate channel occupancy as a fraction of bins above noise floor + margin.
    pub fn channel_occupancy(&self, samples: &[f32], margin_db: f32) -> f32 {
        let spectrum = self.power_spectrum(samples);
        let n = spectrum.len() / 2; // Only positive frequencies
        let threshold = self.noise_floor + margin_db;

        let occupied = spectrum[..n].iter().filter(|&&p| p > threshold).count();

        occupied as f32 / n as f32
    }

    /// Get the frequency resolution in Hz.
    pub fn frequency_resolution(&self) -> f32 {
        self.sample_rate / self.fft_size as f32
    }

    /// Get the averaged power spectrum (if accumulations have been done).
    pub fn averaged_spectrum(&self) -> Vec<f32> {
        if self.accum_count == 0 {
            return self.spectrum_accum.clone();
        }
        self.spectrum_accum
            .iter()
            .map(|&s| s / self.accum_count as f32)
            .collect()
    }

    /// Reset accumulator and noise floor.
    pub fn reset(&mut self) {
        self.spectrum_accum = vec![0.0; self.fft_size];
        self.accum_count = 0;
        self.noise_floor = -100.0;
    }
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
    fn test_power_spectrum_length() {
        let sensor = SpectrumSensor::new(256, 8000.0);
        let samples = vec![0.0f32; 256];
        let spectrum = sensor.power_spectrum(&samples);
        assert_eq!(spectrum.len(), 256);
    }

    #[test]
    fn test_tone_detection() {
        let mut sensor = SpectrumSensor::new(1024, 8000.0);

        // Update noise floor with silence
        let silence = vec![0.0f32; 1024];
        sensor.update(&silence);

        // Generate a 1000 Hz tone
        let tone = generate_tone(1000.0, 8000.0, 1024, 1.0);
        let detected = sensor.detect_tone(&tone, 1000.0, 10.0);
        assert!(detected, "Should detect 1000 Hz tone");
    }

    #[test]
    fn test_noise_floor_estimation() {
        let mut sensor = SpectrumSensor::new(256, 8000.0);

        // Process several blocks of near-silence
        for _ in 0..10 {
            let noise: Vec<f32> = (0..256)
                .map(|_| 0.001 * (rand_like_hash() as f32 / u32::MAX as f32 - 0.5))
                .collect();
            sensor.update(&noise);
        }

        // Noise floor should be very low
        assert!(
            sensor.noise_floor() < -20.0,
            "Noise floor should be low: {}",
            sensor.noise_floor()
        );
    }

    #[test]
    fn test_frequency_resolution() {
        let sensor = SpectrumSensor::new(1024, 8000.0);
        let res = sensor.frequency_resolution();
        assert!((res - 7.8125).abs() < 0.01); // 8000/1024
    }

    #[test]
    fn test_channel_occupancy_silence() {
        let mut sensor = SpectrumSensor::new(256, 8000.0);
        let silence = vec![0.0f32; 256];
        sensor.update(&silence);
        sensor.update(&silence);

        let occupancy = sensor.channel_occupancy(&silence, 10.0);
        assert!(
            occupancy < 0.1,
            "Silence should have low occupancy: {}",
            occupancy
        );
    }

    #[test]
    fn test_reset() {
        let mut sensor = SpectrumSensor::new(256, 8000.0);
        // Use broadband signal so noise floor rises well above -100
        let broadband: Vec<f32> = (0..256)
            .map(|i| (i as f32 * 0.1).sin() + (i as f32 * 0.7).cos() + (i as f32 * 2.3).sin())
            .collect();
        sensor.update(&broadband);
        assert!(sensor.noise_floor() > -100.0);

        sensor.reset();
        assert_eq!(sensor.noise_floor(), -100.0);
        assert_eq!(sensor.accum_count, 0);
    }

    // Simple deterministic hash for test "randomness" (no rand dep in this crate)
    fn rand_like_hash() -> u32 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let val = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut hasher = DefaultHasher::new();
        val.hash(&mut hasher);
        hasher.finish() as u32
    }
}
