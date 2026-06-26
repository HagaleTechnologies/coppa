//! OFDM modulation and demodulation.
//!
//! Implements the OFDM PHY layer with configurable profiles,
//! preamble-based synchronization, pilot-based channel estimation,
//! and MMSE equalization.
//!
//! # Two OFDM stacks (which one is canonical?)
//!
//! This module contains two parallel OFDM implementations. They do not share
//! types, and a reader should know up front which is the real one:
//!
//! - **Coppa stack (canonical / reference path).** [`CoppaProfile`],
//!   [`coppa_modem::CoppaModem`], [`pilots::CoppaPilotPattern`],
//!   [`frame::CoppaHeader`], and [`sync::SchmidlCox`] form the end-to-end PHY
//!   that the engine and `CoppaTransceiver` actually use. This is the path that
//!   carries real traffic; if you want to understand how Coppa transmits, read
//!   `CoppaModem`.
//! - **Generic stack (pedagogical example).** [`OfdmProfile`], [`OfdmModulator`],
//!   [`OfdmDemodulator`], [`modem::OfdmModem`], and
//!   [`equalizer::LinearInterpolationEstimator`] are a simpler, self-contained
//!   single-symbol OFDM implementation kept as a teaching example. It is *not*
//!   on the engine's data path. It is easier to read first because each step
//!   (IFFT + cyclic prefix, FFT, equalize) is isolated and unencumbered by the
//!   framing/sync machinery of the Coppa stack.
//!
//! Both are retained intentionally. Start with the generic stack to learn the
//! mechanics, then read the Coppa stack for the production design.
pub mod coppa_modem;
pub mod cross_frame_interleaver;
pub mod equalizer;
pub mod frame;
pub mod interleaver;
pub mod modem;
pub mod pilots;
pub mod sync;

use num_complex::Complex32;

/// OFDM system profile configuration.
#[derive(Debug, Clone)]
pub struct OfdmProfile {
    /// Human-readable profile name.
    pub name: &'static str,
    /// FFT size (number of subcarriers including guard).
    pub fft_size: usize,
    /// Number of active data subcarriers.
    pub data_carriers: usize,
    /// Number of pilot subcarriers.
    pub pilot_carriers: usize,
    /// Subcarrier spacing in Hz.
    pub subcarrier_spacing: f32,
    /// Cyclic prefix length in samples.
    pub cp_length: usize,
    /// Sample rate in Hz.
    pub sample_rate: f32,
}

impl OfdmProfile {
    /// Standard HF profile: 2,375 Hz bandwidth, 67 data + 9 pilot carriers.
    pub const HF_STANDARD: OfdmProfile = OfdmProfile {
        name: "Standard-2400",
        fft_size: 256,
        data_carriers: 67,
        pilot_carriers: 9,
        subcarrier_spacing: 31.25,
        cp_length: 64,
        sample_rate: 8000.0,
    };

    /// Narrow 200 Hz profile for maximum range / emergency.
    pub const NARROW_200: OfdmProfile = OfdmProfile {
        name: "Narrow-200",
        fft_size: 256,
        data_carriers: 6,
        pilot_carriers: 2,
        subcarrier_spacing: 31.25,
        cp_length: 64,
        sample_rate: 8000.0,
    };

    /// Narrow 500 Hz profile for robust HF.
    pub const NARROW_500: OfdmProfile = OfdmProfile {
        name: "Narrow-500",
        fft_size: 256,
        data_carriers: 14,
        pilot_carriers: 2,
        subcarrier_spacing: 31.25,
        cp_length: 64,
        sample_rate: 8000.0,
    };

    /// Wide VHF/UHF profile.
    /// Active carriers limited to N/2 - 1 = 63 to fit Hermitian symmetry packing.
    pub const WIDE_VHF: OfdmProfile = OfdmProfile {
        name: "Wide-VHF",
        fft_size: 128,
        data_carriers: 55,
        pilot_carriers: 8,
        subcarrier_spacing: 375.0,
        cp_length: 4,
        sample_rate: 48000.0,
    };

    /// Total number of active subcarriers (data + pilot).
    ///
    /// # Panics
    /// Panics if `active_carriers >= fft_size / 2`, which would violate
    /// Hermitian symmetry packing constraints for real-valued output.
    pub fn active_carriers(&self) -> usize {
        let n = self.data_carriers + self.pilot_carriers;
        assert!(
            n < self.fft_size / 2,
            "active_carriers ({}) must be < fft_size/2 ({}) for Hermitian symmetry",
            n,
            self.fft_size / 2,
        );
        n
    }

    /// Useful symbol duration in seconds.
    pub fn symbol_duration(&self) -> f32 {
        self.fft_size as f32 / self.sample_rate
    }

    /// Total symbol duration including CP, in seconds.
    pub fn total_symbol_duration(&self) -> f32 {
        (self.fft_size + self.cp_length) as f32 / self.sample_rate
    }

    /// Symbols per second.
    pub fn symbol_rate(&self) -> f32 {
        1.0 / self.total_symbol_duration()
    }

    /// Signal bandwidth in Hz.
    pub fn bandwidth(&self) -> f32 {
        self.active_carriers() as f32 * self.subcarrier_spacing
    }
}

/// Coppa Protocol OFDM profile configuration.
///
/// Defines the physical-layer parameters for HF and VHF operating modes,
/// including FFT geometry, cyclic prefix, carrier allocation, and protocol
/// identifiers used in the Coppa framing header.
#[derive(Debug, Clone)]
pub struct CoppaProfile {
    /// FFT size (total number of subcarriers including guard bands).
    pub fft_size: usize,
    /// ADC/DAC sample rate in Hz.
    pub sample_rate: u32,
    /// Cyclic prefix length in samples.
    pub cp_samples: usize,
    /// Number of data-bearing subcarriers.
    pub data_carriers: usize,
    /// Number of pilot subcarriers used for channel estimation.
    pub pilot_carriers: usize,
    /// PHY mode identifier (0 = HF, 1 = VHF).
    pub phy_mode: u8,
    /// Bandwidth class identifier encoded in the Coppa frame header.
    pub bandwidth_id: u8,
}

impl CoppaProfile {
    /// HF standard profile: 50 Hz carrier spacing, 48 active carriers, 2.5 kHz class.
    pub fn hf_standard() -> Self {
        Self {
            fft_size: 960,
            sample_rate: 48_000,
            cp_samples: 300,
            data_carriers: 44,
            pilot_carriers: 4,
            phy_mode: 0,
            bandwidth_id: 1,
        }
    }

    /// HF robust profile: same 48 active carriers / 50 Hz spacing as `hf_standard`, but with
    /// 12 pilots (200 Hz spacing) instead of 4, to resolve frequency-selective HF multipath
    /// (the equal-power two-tap Watterson channel's coherence bandwidth is ~500 Hz on Poor).
    /// Trades ~18% of data carriers (36 vs 44) for channel-estimation accuracy.
    pub fn hf_robust() -> Self {
        Self {
            fft_size: 960,
            sample_rate: 48_000,
            cp_samples: 300,
            data_carriers: 36,
            pilot_carriers: 12,
            phy_mode: 0,
            bandwidth_id: 3,
        }
    }

    /// HF narrow profile: reduced carrier count for maximum range / congested bands.
    pub fn hf_narrow() -> Self {
        Self {
            fft_size: 960,
            sample_rate: 48_000,
            cp_samples: 300,
            data_carriers: 8,
            pilot_carriers: 2,
            phy_mode: 0,
            bandwidth_id: 0,
        }
    }

    /// HF wide profile: maximum HF throughput.
    pub fn hf_wide() -> Self {
        Self {
            fft_size: 960,
            sample_rate: 48_000,
            cp_samples: 300,
            data_carriers: 50,
            pilot_carriers: 4,
            phy_mode: 0,
            bandwidth_id: 2,
        }
    }

    /// VHF narrow profile: shorter CP for reduced multipath, standard carrier count.
    pub fn vhf_narrow() -> Self {
        Self {
            fft_size: 960,
            sample_rate: 48_000,
            cp_samples: 60,
            data_carriers: 44,
            pilot_carriers: 4,
            phy_mode: 1,
            bandwidth_id: 1,
        }
    }

    /// VHF wide profile: maximum VHF throughput with expanded carrier set.
    pub fn vhf_wide() -> Self {
        Self {
            fft_size: 960,
            sample_rate: 48_000,
            cp_samples: 60,
            data_carriers: 104,
            pilot_carriers: 8,
            phy_mode: 1,
            bandwidth_id: 2,
        }
    }

    /// Subcarrier spacing in Hz: sample_rate / fft_size.
    pub fn carrier_spacing_hz(&self) -> f32 {
        self.sample_rate as f32 / self.fft_size as f32
    }

    /// Total number of active subcarriers (data + pilot).
    pub fn total_active_carriers(&self) -> usize {
        self.data_carriers + self.pilot_carriers
    }

    /// Total OFDM symbol duration including cyclic prefix, in milliseconds.
    pub fn symbol_duration_ms(&self) -> f32 {
        (self.fft_size + self.cp_samples) as f32 / self.sample_rate as f32 * 1000.0
    }

    /// Useful (data-bearing) symbol duration excluding cyclic prefix, in milliseconds.
    pub fn useful_symbol_ms(&self) -> f32 {
        self.fft_size as f32 / self.sample_rate as f32 * 1000.0
    }

    /// Number of complete OFDM symbols per second.
    pub fn symbols_per_second(&self) -> f32 {
        self.sample_rate as f32 / (self.fft_size + self.cp_samples) as f32
    }
}

/// Modulation and Coding Scheme entry.
#[derive(Debug, Clone, Copy)]
pub struct McsEntry {
    /// MCS index (0-10).
    pub index: u8,
    /// Modulation order (bits per symbol).
    pub bits_per_symbol: usize,
    /// FEC code rate.
    pub code_rate: f32,
    /// Minimum required SNR in dB.
    pub min_snr_db: f32,
}

/// MCS table for Standard-2400 profile.
pub const MCS_TABLE: [McsEntry; 11] = [
    McsEntry {
        index: 0,
        bits_per_symbol: 1,
        code_rate: 0.25,
        min_snr_db: -1.0,
    },
    McsEntry {
        index: 1,
        bits_per_symbol: 1,
        code_rate: 0.5,
        min_snr_db: 3.0,
    },
    McsEntry {
        index: 2,
        bits_per_symbol: 1,
        code_rate: 0.75,
        min_snr_db: 5.0,
    },
    McsEntry {
        index: 3,
        bits_per_symbol: 2,
        code_rate: 0.5,
        min_snr_db: 6.0,
    },
    McsEntry {
        index: 4,
        bits_per_symbol: 2,
        code_rate: 0.75,
        min_snr_db: 9.0,
    },
    McsEntry {
        index: 5,
        bits_per_symbol: 3,
        code_rate: 0.6667,
        min_snr_db: 12.0,
    },
    McsEntry {
        index: 6,
        bits_per_symbol: 4,
        code_rate: 0.5,
        min_snr_db: 13.0,
    },
    McsEntry {
        index: 7,
        bits_per_symbol: 4,
        code_rate: 0.75,
        min_snr_db: 18.0,
    },
    McsEntry {
        index: 8,
        bits_per_symbol: 6,
        code_rate: 0.6667,
        min_snr_db: 21.0,
    },
    McsEntry {
        index: 9,
        bits_per_symbol: 6,
        code_rate: 0.75,
        min_snr_db: 24.0,
    },
    McsEntry {
        index: 10,
        bits_per_symbol: 6,
        code_rate: 0.875,
        min_snr_db: 27.0,
    },
];

/// Clip OFDM signal peaks to reduce PAPR.
/// `target_papr_db`: maximum peak-to-RMS ratio in dB (typically 7.0).
pub fn papr_clip(signal: &[f32], target_papr_db: f32) -> Vec<f32> {
    if signal.is_empty() {
        return Vec::new();
    }

    // Compute RMS
    let mean_sq = signal.iter().map(|&s| s * s).sum::<f32>() / signal.len() as f32;
    let rms = mean_sq.sqrt();

    // Return unmodified for near-zero RMS to avoid division by zero
    if rms < 1e-10 {
        return signal.to_vec();
    }

    // threshold = RMS × 10^(target_papr_db / 20)
    let threshold = rms * 10.0f32.powf(target_papr_db / 20.0);

    signal
        .iter()
        .map(|&s| {
            if s.abs() > threshold {
                threshold * s.signum()
            } else {
                s
            }
        })
        .collect()
}

impl McsEntry {
    /// Net data rate in bps for the given profile.
    pub fn data_rate(&self, profile: &OfdmProfile) -> f32 {
        profile.data_carriers as f32
            * self.bits_per_symbol as f32
            * self.code_rate
            * profile.symbol_rate()
    }
}

/// OFDM modulator: maps frequency-domain symbols to time-domain samples.
///
/// Modulating subcarriers to time-domain samples and then demodulating the
/// payload (after stripping the cyclic prefix) recovers the original
/// subcarriers, up to FFT precision:
///
/// ```
/// use coppa_codec::ofdm::{OfdmModulator, OfdmDemodulator, OfdmProfile};
/// use num_complex::Complex32;
///
/// let profile = OfdmProfile::HF_STANDARD;
/// let modulator = OfdmModulator::new(profile.clone());
/// let demodulator = OfdmDemodulator::new(profile.clone());
///
/// // BPSK-style subcarriers (real +/-1).
/// let n = profile.active_carriers();
/// let tx: Vec<Complex32> = (0..n)
///     .map(|i| Complex32::new(if i % 2 == 0 { 1.0 } else { -1.0 }, 0.0))
///     .collect();
///
/// let samples = modulator.modulate_symbol(&tx);
/// assert_eq!(samples.len(), profile.fft_size + profile.cp_length);
///
/// // Drop the cyclic prefix, then demodulate.
/// let rx = demodulator.demodulate_symbol(&samples[profile.cp_length..]);
/// for (a, b) in tx.iter().zip(rx.iter()) {
///     assert!((a - b).norm() < 0.01);
/// }
/// ```
pub struct OfdmModulator {
    profile: OfdmProfile,
    fft: coppa_dsp::fft::FftProcessor,
}

impl OfdmModulator {
    pub fn new(profile: OfdmProfile) -> Self {
        let fft = coppa_dsp::fft::FftProcessor::new(profile.fft_size);
        Self { profile, fft }
    }

    /// Modulate one OFDM symbol from frequency-domain subcarrier values.
    /// Returns real-valued time-domain samples with cyclic prefix prepended.
    ///
    /// Uses Hermitian symmetry: subcarrier[i] placed at bin i+1 (positive freq),
    /// conj(subcarrier[i]) placed at bin N-i-1 (negative freq). This ensures
    /// the IFFT output is real-valued for transmission over audio channels.
    pub fn modulate_symbol(&self, subcarriers: &[Complex32]) -> Vec<f32> {
        let n = self.profile.fft_size;

        let mut freq = vec![Complex32::new(0.0, 0.0); n];
        // Limit active carriers to N/2 - 1 to prevent Hermitian packing collision
        let max_hermitian = n / 2 - 1;
        let n_active = subcarriers
            .len()
            .min(self.profile.active_carriers())
            .min(max_hermitian);

        // Place on positive frequencies (bins 1..n_active) and mirror
        // conjugates on negative frequencies (bins N-1..N-n_active)
        for (i, &sc) in subcarriers[..n_active].iter().enumerate() {
            let bin_pos = i + 1;
            let bin_neg = n - i - 1;
            if bin_pos < n && bin_pos != bin_neg {
                freq[bin_pos] = sc;
                freq[bin_neg] = sc.conj();
            } else if bin_pos < n {
                // Nyquist bin: must be real
                freq[bin_pos] = Complex32::new(sc.re, 0.0);
            }
        }

        let time = self.fft.inverse(&freq);

        // Add cyclic prefix
        let cp_start = n - self.profile.cp_length;
        let mut output = Vec::with_capacity(n + self.profile.cp_length);
        output.extend(time[cp_start..].iter().map(|s| s.re));
        output.extend(time.iter().map(|s| s.re));

        output
    }

    pub fn profile(&self) -> &OfdmProfile {
        &self.profile
    }
}

/// OFDM demodulator: extracts frequency-domain symbols from time-domain samples.
pub struct OfdmDemodulator {
    profile: OfdmProfile,
    fft: coppa_dsp::fft::FftProcessor,
}

impl OfdmDemodulator {
    pub fn new(profile: OfdmProfile) -> Self {
        let fft = coppa_dsp::fft::FftProcessor::new(profile.fft_size);
        Self { profile, fft }
    }

    /// Demodulate one OFDM symbol from time-domain samples (with CP removed).
    /// Input should be exactly `fft_size` samples.
    /// Returns the active subcarrier values extracted from positive frequency bins.
    pub fn demodulate_symbol(&self, samples: &[f32]) -> Vec<Complex32> {
        let input: Vec<Complex32> = samples
            .iter()
            .take(self.profile.fft_size)
            .map(|&s| Complex32::new(s, 0.0))
            .collect();

        let freq = self.fft.forward(&input);

        // Extract active subcarriers from positive frequency bins 1..n_active
        let n_active = self.profile.active_carriers();
        let mut output = Vec::with_capacity(n_active);

        for i in 0..n_active {
            output.push(freq[i + 1]); // skip DC
        }

        output
    }

    pub fn profile(&self) -> &OfdmProfile {
        &self.profile
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hf_robust_has_dense_pilots() {
        let p = CoppaProfile::hf_robust();
        assert_eq!(
            p.total_active_carriers(),
            48,
            "same active-carrier count as hf_standard"
        );
        assert_eq!(p.data_carriers, 36);
        assert_eq!(p.pilot_carriers, 12);
        // Per-symbol pilot spacing = 48 / 12 = 4 carriers = 200 Hz at 50 Hz spacing,
        // below Poor's ~500 Hz coherence bandwidth.
        assert_eq!(p.total_active_carriers() / p.pilot_carriers, 4);
    }

    #[test]
    fn test_ofdm_profile_calculations() {
        let p = OfdmProfile::HF_STANDARD;
        assert_eq!(p.active_carriers(), 76);
        assert!((p.symbol_duration() - 0.032).abs() < 0.001);
        assert!((p.total_symbol_duration() - 0.040).abs() < 0.001);
        assert!((p.symbol_rate() - 25.0).abs() < 0.1);
        assert!((p.bandwidth() - 2375.0).abs() < 1.0);
    }

    #[test]
    fn test_mcs_data_rates() {
        let profile = OfdmProfile::HF_STANDARD;
        let rate_0 = MCS_TABLE[0].data_rate(&profile);
        let rate_10 = MCS_TABLE[10].data_rate(&profile);
        assert!(rate_0 > 400.0 && rate_0 < 450.0, "MCS 0 rate: {}", rate_0);
        assert!(
            rate_10 > 8700.0 && rate_10 < 8900.0,
            "MCS 10 rate: {}",
            rate_10
        );
    }

    #[test]
    fn test_ofdm_modulate_demodulate_roundtrip() {
        let profile = OfdmProfile::HF_STANDARD;
        let modulator = OfdmModulator::new(profile.clone());
        let demodulator = OfdmDemodulator::new(profile.clone());

        // Create test subcarriers
        let n_active = profile.active_carriers();
        let subcarriers: Vec<Complex32> = (0..n_active)
            .map(|i| Complex32::new(if i % 2 == 0 { 1.0 } else { -1.0 }, 0.0))
            .collect();

        // Modulate
        let samples = modulator.modulate_symbol(&subcarriers);
        assert_eq!(samples.len(), profile.fft_size + profile.cp_length);

        // Remove CP and demodulate
        let data_samples = &samples[profile.cp_length..];
        let recovered = demodulator.demodulate_symbol(data_samples);

        assert_eq!(recovered.len(), n_active);

        // Check recovery (allowing for FFT precision)
        for (i, (orig, recv)) in subcarriers.iter().zip(recovered.iter()).enumerate() {
            assert!(
                (orig - recv).norm() < 0.01,
                "Subcarrier {} mismatch: orig={:?}, recv={:?}",
                i,
                orig,
                recv
            );
        }
    }

    #[test]
    fn test_ofdm_symbol_sample_count() {
        let profile = OfdmProfile::HF_STANDARD;
        let modulator = OfdmModulator::new(profile.clone());
        let subcarriers = vec![Complex32::new(1.0, 0.0); profile.active_carriers()];
        let samples = modulator.modulate_symbol(&subcarriers);
        // FFT size + CP length
        assert_eq!(samples.len(), 256 + 64);
    }

    #[test]
    fn test_ofdm_narrow_200_profile() {
        let p = OfdmProfile::NARROW_200;
        assert_eq!(p.active_carriers(), 8); // 6 data + 2 pilot
        assert!((p.bandwidth() - 250.0).abs() < 1.0);
    }

    #[test]
    fn test_ofdm_narrow_500_profile() {
        let p = OfdmProfile::NARROW_500;
        assert_eq!(p.active_carriers(), 16); // 14 data + 2 pilot
        assert!((p.bandwidth() - 500.0).abs() < 1.0);
    }

    #[test]
    fn test_ofdm_wide_vhf_profile() {
        let p = OfdmProfile::WIDE_VHF;
        assert_eq!(p.active_carriers(), 63); // 55 data + 8 pilot
                                             // Verify active carriers fit in Hermitian packing (must be < N/2)
        assert!(p.active_carriers() < p.fft_size / 2);
        assert!((p.bandwidth() - 23625.0).abs() < 1.0);
    }

    #[test]
    fn test_ofdm_roundtrip_narrow_profile() {
        let profile = OfdmProfile::NARROW_200;
        let modulator = OfdmModulator::new(profile.clone());
        let demodulator = OfdmDemodulator::new(profile.clone());

        let n_active = profile.active_carriers();
        let subcarriers: Vec<Complex32> = (0..n_active)
            .map(|i| Complex32::new(if i % 2 == 0 { 1.0 } else { -1.0 }, 0.0))
            .collect();

        let samples = modulator.modulate_symbol(&subcarriers);
        let data_samples = &samples[profile.cp_length..];
        let recovered = demodulator.demodulate_symbol(data_samples);

        assert_eq!(recovered.len(), n_active);
        for (i, (orig, recv)) in subcarriers.iter().zip(recovered.iter()).enumerate() {
            assert!(
                (orig - recv).norm() < 0.01,
                "Narrow profile subcarrier {} mismatch",
                i
            );
        }
    }

    #[test]
    fn test_ofdm_modulate_produces_real_output() {
        // With Hermitian symmetry, IFFT output should be real-valued
        let profile = OfdmProfile::HF_STANDARD;
        let modulator = OfdmModulator::new(profile.clone());
        let subcarriers: Vec<Complex32> = (0..profile.active_carriers())
            .map(|i| Complex32::new((i as f32 * 0.7).cos(), (i as f32 * 0.7).sin()))
            .collect();

        let samples = modulator.modulate_symbol(&subcarriers);
        // All samples should be real (f32), which they are by construction
        // Check they're finite and not NaN
        for (i, &s) in samples.iter().enumerate() {
            assert!(s.is_finite(), "Sample {} should be finite, got {}", i, s);
        }
    }

    #[test]
    fn test_mcs_rates_monotonic() {
        // Higher MCS indices should require higher SNR
        for i in 1..MCS_TABLE.len() {
            assert!(
                MCS_TABLE[i].min_snr_db >= MCS_TABLE[i - 1].min_snr_db,
                "MCS {} SNR {} should be >= MCS {} SNR {}",
                i,
                MCS_TABLE[i].min_snr_db,
                i - 1,
                MCS_TABLE[i - 1].min_snr_db
            );
        }
    }

    #[test]
    fn test_ofdm_roundtrip_with_awgn() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let profile = OfdmProfile::HF_STANDARD;
        let modulator = OfdmModulator::new(profile.clone());
        let demodulator = OfdmDemodulator::new(profile.clone());

        let n_active = profile.active_carriers();
        // BPSK-like subcarriers (real ±1)
        let subcarriers: Vec<Complex32> = (0..n_active)
            .map(|i| Complex32::new(if i % 2 == 0 { 1.0 } else { -1.0 }, 0.0))
            .collect();

        let samples = modulator.modulate_symbol(&subcarriers);

        // Add AWGN at 20 dB SNR (should be easily recoverable)
        let signal_power: f32 = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
        let snr_linear = 100.0f32; // 20 dB
        let noise_std = (signal_power / snr_linear).sqrt();
        let mut rng = StdRng::seed_from_u64(123);

        let noisy_samples: Vec<f32> = samples
            .iter()
            .map(|&s| {
                let u1: f32 = rng.random::<f32>().max(1e-10);
                let u2: f32 = rng.random();
                let noise =
                    noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                s + noise
            })
            .collect();

        // Remove CP and demodulate
        let data_samples = &noisy_samples[profile.cp_length..];
        let recovered = demodulator.demodulate_symbol(data_samples);

        assert_eq!(recovered.len(), n_active);

        // At 20 dB SNR, subcarrier error should be small
        let mut max_error = 0.0f32;
        for (orig, recv) in subcarriers.iter().zip(recovered.iter()) {
            let err = (orig - recv).norm();
            max_error = max_error.max(err);
        }
        assert!(
            max_error < 0.5,
            "Max subcarrier error at 20dB SNR should be < 0.5, got {}",
            max_error
        );
    }

    #[test]
    fn test_coppa_hf_standard_profile() {
        let p = CoppaProfile::hf_standard();
        assert_eq!(p.fft_size, 960);
        assert_eq!(p.sample_rate, 48_000);
        assert_eq!(p.cp_samples, 300);
        assert_eq!(p.data_carriers, 44);
        assert_eq!(p.pilot_carriers, 4);
        assert_eq!(p.phy_mode, 0);
        assert_eq!(p.bandwidth_id, 1);
        assert!(
            (p.carrier_spacing_hz() - 50.0).abs() < 0.01,
            "Expected 50 Hz spacing, got {}",
            p.carrier_spacing_hz()
        );
        assert!(
            (p.symbol_duration_ms() - 26.25).abs() < 0.01,
            "Expected 26.25 ms symbol duration, got {}",
            p.symbol_duration_ms()
        );
    }

    #[test]
    fn test_coppa_hf_narrow_profile() {
        let p = CoppaProfile::hf_narrow();
        assert_eq!(p.data_carriers, 8);
        assert_eq!(p.pilot_carriers, 2);
        assert_eq!(p.total_active_carriers(), 10);
        assert_eq!(p.bandwidth_id, 0);
    }

    #[test]
    fn test_coppa_hf_wide_profile() {
        let p = CoppaProfile::hf_wide();
        assert_eq!(p.data_carriers, 50);
        assert_eq!(p.pilot_carriers, 4);
        assert_eq!(p.total_active_carriers(), 54);
        assert_eq!(p.bandwidth_id, 2);
    }

    #[test]
    fn test_coppa_vhf_narrow_profile() {
        let p = CoppaProfile::vhf_narrow();
        assert_eq!(p.cp_samples, 60);
        assert_eq!(p.phy_mode, 1);
        assert_eq!(p.bandwidth_id, 1);
        // symbol_duration_ms = (960 + 60) / 48000 * 1000 = 1020 / 48000 * 1000 = 21.25
        assert!(
            (p.symbol_duration_ms() - 21.25).abs() < 0.01,
            "Expected 21.25 ms symbol duration, got {}",
            p.symbol_duration_ms()
        );
    }

    #[test]
    fn test_coppa_vhf_wide_profile() {
        let p = CoppaProfile::vhf_wide();
        assert_eq!(p.data_carriers, 104);
        assert_eq!(p.pilot_carriers, 8);
        assert_eq!(p.total_active_carriers(), 112);
        assert_eq!(p.phy_mode, 1);
        assert_eq!(p.bandwidth_id, 2);
    }

    #[test]
    fn test_papr_clip_reduces_peaks() {
        // Baseline signal of 1.0, then a spike of 100.0
        let mut signal: Vec<f32> = vec![1.0f32; 99];
        signal.push(100.0);

        let clipped = papr_clip(&signal, 7.0);
        assert_eq!(clipped.len(), signal.len());

        // The spike should be reduced
        assert!(
            clipped[99] < 100.0,
            "Spike should be clipped, got {}",
            clipped[99]
        );

        // Normal samples close to 1.0 should be unchanged
        for &s in &clipped[..99] {
            assert!(
                (s - 1.0).abs() < 1e-5,
                "Normal sample should be ~1.0, got {}",
                s
            );
        }
    }

    #[test]
    fn test_papr_clip_at_7db() {
        // Signal: 100,000 alternating ±1.0 samples (RMS = 1.0) plus two large spikes.
        // With many baseline samples the spikes barely affect the pre-clip RMS,
        // so threshold ≈ 1.0 × 10^(7/20) ≈ 2.238. After clipping both spikes, the
        // output peak is ~2.238 and RMS stays near 1.0 → PAPR ≈ 7 dB ≤ 8 dB.
        let n = 100_000usize;
        let mut signal: Vec<f32> = (0..n)
            .map(|i| if i % 2 == 0 { 1.0f32 } else { -1.0f32 })
            .collect();
        // Replace two samples with large spikes — they contribute negligibly to RMS
        signal[0] = 50.0;
        signal[n / 2] = -50.0;

        let clipped = papr_clip(&signal, 7.0);

        // Compute output RMS and peak
        let rms = (clipped.iter().map(|&s| s * s).sum::<f32>() / clipped.len() as f32).sqrt();
        let peak = clipped.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

        assert!(rms > 1e-10, "RMS should be non-zero");

        let papr_db = 20.0 * (peak / rms).log10();
        assert!(
            papr_db <= 8.0,
            "Output PAPR should be ≤ 8 dB after clipping at 7 dB, got {} dB",
            papr_db
        );
    }
}
