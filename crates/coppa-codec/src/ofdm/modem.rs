//! OFDM modem implementing the `Modem` trait.
//!
//! Wraps `OfdmModulator` and `OfdmDemodulator` to provide a standard
//! `Modem` interface using BPSK subcarrier mapping. This is part of the
//! generic (pedagogical) OFDM stack, not the canonical Coppa data path; see
//! the [`crate::ofdm`] module docs for the distinction.
use anyhow::Result;
use num_complex::Complex32;

use super::equalizer::{mmse_equalize, LinearInterpolationEstimator};
use super::pilots::PilotPattern;
use super::sync::SchmidlCox;
use super::{OfdmDemodulator, OfdmModulator, OfdmProfile};
use crate::traits::{ChannelEstimator, Modem};

/// OFDM modem that implements the `Modem` trait.
///
/// Uses BPSK mapping on each data subcarrier: bit 0 -> +1, bit 1 -> -1.
/// TX prepends a Schmidl-Cox STS (Short Training Sequence) for frame detection.
/// RX uses SchmidlCox::detect() to find the frame start before demodulating.
/// Pilot-based MMSE equalization is applied on each received OFDM symbol.
pub struct OfdmModem {
    profile: OfdmProfile,
    modulator: OfdmModulator,
    demodulator: OfdmDemodulator,
    pilot_pattern: PilotPattern,
    sync: SchmidlCox,
    channel_estimator: LinearInterpolationEstimator,
}

impl OfdmModem {
    /// Create a new OFDM modem with the given profile.
    pub fn new(profile: OfdmProfile) -> Self {
        let modulator = OfdmModulator::new(profile.clone());
        let demodulator = OfdmDemodulator::new(profile.clone());
        let num_active = profile.active_carriers();
        let pilot_pattern = PilotPattern::new(num_active, profile.pilot_carriers);
        let sync = SchmidlCox::new(profile.fft_size, profile.cp_length).with_threshold(0.5);
        let channel_estimator = LinearInterpolationEstimator::new(num_active);
        Self {
            profile,
            modulator,
            demodulator,
            pilot_pattern,
            sync,
            channel_estimator,
        }
    }

    /// Generate a Schmidl-Cox Short Training Sequence (STS).
    ///
    /// The STS consists of two identical halves in the time domain, created by
    /// populating only even-indexed subcarriers in the frequency domain.
    /// A cyclic prefix is prepended for consistency with data symbols.
    fn generate_sts(&self) -> Vec<f32> {
        let fft_size = self.profile.fft_size;
        let half = fft_size / 2;

        // Populate even subcarriers with a known PN sequence for the STS.
        // Using even subcarriers creates a time-domain signal with two identical halves.
        let mut freq = vec![Complex32::new(0.0, 0.0); fft_size];
        let active = self.profile.active_carriers();
        for k in 0..active {
            if k % 2 == 0 {
                // Simple PN: alternating +1/-1 on even subcarriers
                let val = if (k / 2) % 2 == 0 { 1.0 } else { -1.0 };
                let bin = k + 1; // skip DC
                if bin < half {
                    freq[bin] = Complex32::new(val, 0.0);
                    // Hermitian symmetry for real output
                    freq[fft_size - bin] = Complex32::new(val, 0.0);
                }
            }
        }

        // IFFT to get time-domain STS
        let fft_proc = coppa_dsp::fft::FftProcessor::new(fft_size);
        let time = fft_proc.inverse(&freq);

        // Prepend cyclic prefix
        let cp_len = self.profile.cp_length;
        let mut sts_samples = Vec::with_capacity(cp_len + fft_size);
        sts_samples.extend(time[(fft_size - cp_len)..].iter().map(|s| s.re));
        sts_samples.extend(time.iter().map(|s| s.re));
        sts_samples
    }

    /// Number of data bits that fit in one OFDM symbol (BPSK: 1 bit per subcarrier).
    fn bits_per_symbol(&self) -> usize {
        self.pilot_pattern.data_indices.len()
    }

    /// Total samples per OFDM symbol (FFT size + cyclic prefix).
    fn ofdm_symbol_samples(&self) -> usize {
        self.profile.fft_size + self.profile.cp_length
    }
}

impl Modem for OfdmModem {
    fn modulate(&self, bits: &[u8]) -> Result<Vec<f32>> {
        let bps = self.bits_per_symbol();
        if bps == 0 || bits.is_empty() {
            return Ok(Vec::new());
        }

        let n_symbols = bits.len().div_ceil(bps);
        // Prepend STS for frame detection
        let sts = self.generate_sts();
        let mut all_samples =
            Vec::with_capacity(sts.len() + n_symbols * self.ofdm_symbol_samples());
        all_samples.extend(sts);

        for chunk in bits.chunks(bps) {
            // Map data bits to BPSK symbols
            let data_symbols: Vec<Complex32> = (0..bps)
                .map(|i| {
                    let val = if i < chunk.len() {
                        if chunk[i] == 0 {
                            1.0
                        } else {
                            -1.0
                        }
                    } else {
                        1.0 // Pad with +1 for incomplete last symbol
                    };
                    Complex32::new(val, 0.0)
                })
                .collect();

            // Use PilotPattern to interleave data + pilots
            let subcarriers = self.pilot_pattern.insert(&data_symbols);
            let symbol_samples = self.modulator.modulate_symbol(&subcarriers);
            all_samples.extend(symbol_samples);
        }

        Ok(all_samples)
    }

    fn demodulate_soft(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let sym_len = self.ofdm_symbol_samples();
        let fft_size = self.profile.fft_size;
        let cp_len = self.profile.cp_length;

        // Use Schmidl-Cox to find the STS and determine frame start
        let data_start_offset = match self.sync.detect(samples, self.profile.sample_rate) {
            Some((sts_pos, _cfo)) => {
                // STS is one OFDM symbol (CP + FFT). Data symbols start after it.
                sts_pos + cp_len + fft_size
            }
            None => {
                // No STS detected — fall back to assuming data starts at offset 0
                // (backward compatible with raw OFDM samples without STS)
                0
            }
        };

        let mut soft_bits = Vec::new();

        let mut offset = data_start_offset;
        while offset + sym_len <= samples.len() {
            // Strip cyclic prefix
            let data_start = offset + cp_len;
            let data_end = data_start + fft_size;
            if data_end > samples.len() {
                break;
            }
            let symbol_samples = &samples[data_start..data_end];

            // Demodulate to frequency domain
            let subcarriers = self.demodulator.demodulate_symbol(symbol_samples);

            // Extract pilots and update channel estimator
            let received_pilots = self.pilot_pattern.extract_pilots(&subcarriers);
            let pilot_triples: Vec<(usize, Complex32, Complex32)> = received_pilots
                .iter()
                .enumerate()
                .map(|(i, &(idx, received))| {
                    let known = if i < self.pilot_pattern.pilot_values.len() {
                        self.pilot_pattern.pilot_values[i]
                    } else {
                        Complex32::new(1.0, 0.0)
                    };
                    (idx, received, known)
                })
                .collect();
            self.channel_estimator.update(&pilot_triples);

            // MMSE equalize using channel estimates
            let num_active = self.profile.active_carriers();
            let equalized = mmse_equalize(&subcarriers, &self.channel_estimator, num_active);

            // Extract data subcarriers from equalized vector
            let data_subcarriers = self.pilot_pattern.extract_data(&equalized);
            for sc in &data_subcarriers {
                soft_bits.push(sc.re);
            }

            offset += sym_len;
        }

        Ok(soft_bits)
    }

    fn sample_rate(&self) -> f32 {
        self.profile.sample_rate
    }

    fn samples_per_symbol(&self) -> usize {
        self.ofdm_symbol_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ofdm_modem_roundtrip() {
        let profile = OfdmProfile::HF_STANDARD;
        let mut modem = OfdmModem::new(profile.clone());

        // Create a test bit pattern
        let n_bits = profile.data_carriers * 2; // 2 OFDM symbols worth
        let bits: Vec<u8> = (0..n_bits).map(|i| (i % 2) as u8).collect();

        let samples = modem.modulate(&bits).unwrap();
        assert!(!samples.is_empty());

        let soft = modem.demodulate_soft(&samples).unwrap();
        assert_eq!(soft.len(), n_bits);

        // Check subcarrier recovery: positive soft -> bit 0, negative soft -> bit 1
        let mut correct = 0;
        for (i, &s) in soft.iter().enumerate() {
            let decoded = if s >= 0.0 { 0u8 } else { 1u8 };
            if decoded == bits[i] {
                correct += 1;
            }
        }
        let accuracy = correct as f32 / n_bits as f32;
        assert!(
            accuracy > 0.95,
            "OFDM modem roundtrip accuracy should be > 95%, got {:.1}%",
            accuracy * 100.0
        );
    }

    #[test]
    fn test_ofdm_modem_produces_samples() {
        let modem = OfdmModem::new(OfdmProfile::HF_STANDARD);
        let bits = vec![0, 1, 0, 1, 0, 1, 0, 1];
        let samples = modem.modulate(&bits).unwrap();
        assert!(!samples.is_empty());
        // Should produce at least one OFDM symbol
        assert!(samples.len() >= modem.samples_per_symbol());
    }

    #[test]
    fn test_ofdm_modem_sample_rate() {
        let modem = OfdmModem::new(OfdmProfile::HF_STANDARD);
        assert_eq!(modem.sample_rate(), 8000.0);
        assert_eq!(modem.samples_per_symbol(), 256 + 64);
    }

    #[test]
    fn test_ofdm_modem_empty_input() {
        let modem = OfdmModem::new(OfdmProfile::HF_STANDARD);
        let samples = modem.modulate(&[]).unwrap();
        assert!(samples.is_empty());
    }
}
