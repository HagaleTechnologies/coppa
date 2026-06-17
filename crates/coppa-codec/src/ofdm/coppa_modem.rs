//! Coppa modem: end-to-end OFDM modulate/demodulate pipeline.
//!
//! Assembles preamble + header + payload into audio samples,
//! and demodulates audio back into frames. Supports a BPSK-only path
//! (modulate/demodulate) and a generic constellation-mapped path
//! (modulate_mapped/demodulate_soft) for use with FEC coding.
//!
//! This is the canonical/reference OFDM path used by the engine and
//! `CoppaTransceiver`. See the [`crate::ofdm`] module docs for how it relates
//! to the simpler generic OFDM stack.
use num_complex::Complex32;

use coppa_dsp::fft::FftProcessor;

use super::equalizer::{mmse_equalize, LinearInterpolationEstimator};
use super::frame::CoppaHeader;
use super::papr_clip;
use super::pilots::CoppaPilotPattern;
use super::sync::{detect_coppa_version, generate_coppa_preamble};
use super::CoppaProfile;
use crate::traits::ChannelEstimator;

/// Speed level configuration for future MCS support.
#[derive(Debug, Clone, Copy)]
pub struct SpeedLevel {
    pub level: u8,
    pub bits_per_symbol: u8,
    pub ldpc_rate_num: u8,
    pub ldpc_rate_den: u8,
    pub papr_target_db: f32,
}

pub const SPEED_LEVELS: [SpeedLevel; 9] = [
    SpeedLevel {
        level: 1,
        bits_per_symbol: 1,
        ldpc_rate_num: 1,
        ldpc_rate_den: 4,
        papr_target_db: 6.0,
    },
    SpeedLevel {
        level: 2,
        bits_per_symbol: 1,
        ldpc_rate_num: 1,
        ldpc_rate_den: 2,
        papr_target_db: 6.0,
    },
    SpeedLevel {
        level: 3,
        bits_per_symbol: 2,
        ldpc_rate_num: 1,
        ldpc_rate_den: 2,
        papr_target_db: 7.0,
    },
    SpeedLevel {
        level: 4,
        bits_per_symbol: 2,
        ldpc_rate_num: 3,
        ldpc_rate_den: 4,
        papr_target_db: 7.0,
    },
    SpeedLevel {
        level: 5,
        bits_per_symbol: 3,
        ldpc_rate_num: 2,
        ldpc_rate_den: 3,
        papr_target_db: 8.0,
    },
    SpeedLevel {
        level: 6,
        bits_per_symbol: 4,
        ldpc_rate_num: 1,
        ldpc_rate_den: 2,
        papr_target_db: 9.5,
    },
    SpeedLevel {
        level: 7,
        bits_per_symbol: 4,
        ldpc_rate_num: 3,
        ldpc_rate_den: 4,
        papr_target_db: 11.0,
    },
    SpeedLevel {
        level: 9,
        bits_per_symbol: 6,
        ldpc_rate_num: 2,
        ldpc_rate_den: 3,
        papr_target_db: 11.0,
    },
    SpeedLevel {
        level: 10,
        bits_per_symbol: 6,
        ldpc_rate_num: 7,
        ldpc_rate_den: 8,
        papr_target_db: 14.0,
    },
];

/// High-level OFDM modem for the Coppa Protocol.
pub struct CoppaModem {
    profile: CoppaProfile,
    fft: FftProcessor,
    pilots: CoppaPilotPattern,
    version: u8,
}

impl CoppaModem {
    /// Create a new modem for the given profile and protocol version.
    pub fn new(profile: CoppaProfile, version: u8) -> Self {
        let total_active = profile.total_active_carriers();
        let fft = FftProcessor::new(profile.fft_size);
        let pilots = CoppaPilotPattern::new(total_active, profile.pilot_carriers);
        Self {
            profile,
            fft,
            pilots,
            version,
        }
    }

    /// Number of data carriers per OFDM symbol (excluding pilots).
    fn data_carriers_per_symbol(&self) -> usize {
        self.pilots.num_data()
    }

    /// Build one OFDM symbol from active-carrier complex values.
    ///
    /// Places carriers at bins 1..N with Hermitian symmetry, IFFTs,
    /// and prepends cyclic prefix.
    fn build_ofdm_symbol(&self, active_carriers: &[Complex32]) -> Vec<f32> {
        let n = self.profile.fft_size;
        let cp = self.profile.cp_samples;

        let mut freq = vec![Complex32::new(0.0, 0.0); n];
        for (i, &val) in active_carriers.iter().enumerate() {
            let bin = i + 1; // skip DC at bin 0
            if bin < n / 2 {
                freq[bin] = val;
                freq[n - bin] = val.conj();
            } else if bin == n / 2 {
                freq[bin] = Complex32::new(val.re, 0.0);
            }
        }

        let time = self.fft.inverse(&freq);

        // Prepend cyclic prefix (last cp samples of the symbol)
        let cp_start = n - cp;
        let mut output = Vec::with_capacity(n + cp);
        output.extend(time[cp_start..].iter().map(|s| s.re));
        output.extend(time.iter().map(|s| s.re));
        output
    }

    /// Demodulate one OFDM symbol: strip CP, FFT, extract active carriers.
    fn demod_ofdm_symbol(&self, samples: &[f32]) -> Vec<Complex32> {
        let n = self.profile.fft_size;
        let cp = self.profile.cp_samples;

        if samples.len() < n + cp {
            return vec![];
        }

        // Strip cyclic prefix
        let data = &samples[cp..cp + n];
        let input: Vec<Complex32> = data.iter().map(|&s| Complex32::new(s, 0.0)).collect();
        let freq = self.fft.forward(&input);

        // Extract active carriers from positive frequency bins 1..total_active
        let total_active = self.profile.total_active_carriers();
        (0..total_active).map(|i| freq[i + 1]).collect()
    }

    /// Modulate a frame (header + payload) into audio samples.
    ///
    /// Structure: [preamble (2 sync symbols)] [fine sync symbol] [header symbols] [payload symbols]
    /// All symbols use BPSK mapping: bit 0 -> +1, bit 1 -> -1.
    pub fn modulate(&self, header: &CoppaHeader, payload: &[u8]) -> Vec<f32> {
        let total_active = self.profile.total_active_carriers();
        let data_per_sym = self.data_carriers_per_symbol();

        // 1. Generate preamble (2 Schmidl-Cox sync symbols)
        let mut samples = generate_coppa_preamble(&self.profile, self.version);

        // 2. Generate fine sync symbol: known BPSK +1 on all active carriers
        let fine_sync_carriers = vec![Complex32::new(1.0, 0.0); total_active];
        samples.extend(self.build_ofdm_symbol(&fine_sync_carriers));

        // 3. Encode header bits and modulate as BPSK OFDM symbols
        let header_bits = header.to_bits();
        let header_bpsk: Vec<Complex32> = header_bits
            .iter()
            .map(|&b| {
                if b == 0 {
                    Complex32::new(1.0, 0.0)
                } else {
                    Complex32::new(-1.0, 0.0)
                }
            })
            .collect();

        let num_header_syms = header_bpsk.len().div_ceil(data_per_sym);
        for sym_idx in 0..num_header_syms {
            let start = sym_idx * data_per_sym;
            let end = (start + data_per_sym).min(header_bpsk.len());
            let mut data = header_bpsk[start..end].to_vec();
            // Pad remaining data carriers with +1
            data.resize(data_per_sym, Complex32::new(1.0, 0.0));
            // Insert pilots and build symbol
            // Symbol numbering for pilot alternation: header symbols start at 0
            let carriers = self.pilots.insert_pilots(&data, sym_idx);
            samples.extend(self.build_ofdm_symbol(&carriers));
        }

        // 4. Encode payload as BPSK (MSB first)
        let mut payload_bits = Vec::with_capacity(payload.len() * 8);
        for &byte in payload {
            for shift in (0..8).rev() {
                payload_bits.push((byte >> shift) & 1);
            }
        }

        let payload_bpsk: Vec<Complex32> = payload_bits
            .iter()
            .map(|&b| {
                if b == 0 {
                    Complex32::new(1.0, 0.0)
                } else {
                    Complex32::new(-1.0, 0.0)
                }
            })
            .collect();

        let num_payload_syms = payload_bpsk.len().div_ceil(data_per_sym);
        for sym_idx in 0..num_payload_syms {
            let start = sym_idx * data_per_sym;
            let end = (start + data_per_sym).min(payload_bpsk.len());
            let mut data = payload_bpsk[start..end].to_vec();
            data.resize(data_per_sym, Complex32::new(1.0, 0.0));
            // Continue symbol numbering from header
            let global_sym = num_header_syms + sym_idx;
            let carriers = self.pilots.insert_pilots(&data, global_sym);
            samples.extend(self.build_ofdm_symbol(&carriers));
        }

        // 5. Apply PAPR clipping (BPSK-only path; matches SPEED_LEVELS levels 1-2)
        papr_clip(&samples, 6.0)
    }

    /// Modulate a frame with pre-mapped Complex32 payload symbols.
    ///
    /// The header is still encoded as raw BPSK. Payload symbols are already
    /// constellation-mapped by the caller (e.g., CoppaTransceiver after LDPC
    /// encoding and interleaving).
    pub fn modulate_mapped(
        &self,
        header: &CoppaHeader,
        payload_symbols: &[Complex32],
        papr_target_db: f32,
    ) -> Vec<f32> {
        let total_active = self.profile.total_active_carriers();
        let data_per_sym = self.data_carriers_per_symbol();

        // 1. Preamble
        let mut samples = generate_coppa_preamble(&self.profile, self.version);

        // 2. Fine sync symbol
        let fine_sync_carriers = vec![Complex32::new(1.0, 0.0); total_active];
        samples.extend(self.build_ofdm_symbol(&fine_sync_carriers));

        // 3. Header as BPSK (same as modulate())
        let header_bits = header.to_bits();
        let header_bpsk: Vec<Complex32> = header_bits
            .iter()
            .map(|&b| {
                if b == 0 {
                    Complex32::new(1.0, 0.0)
                } else {
                    Complex32::new(-1.0, 0.0)
                }
            })
            .collect();

        let num_header_syms = header_bpsk.len().div_ceil(data_per_sym);
        for sym_idx in 0..num_header_syms {
            let start = sym_idx * data_per_sym;
            let end = (start + data_per_sym).min(header_bpsk.len());
            let mut data = header_bpsk[start..end].to_vec();
            data.resize(data_per_sym, Complex32::new(1.0, 0.0));
            let carriers = self.pilots.insert_pilots(&data, sym_idx);
            samples.extend(self.build_ofdm_symbol(&carriers));
        }

        // 4. Payload: pack pre-mapped symbols into OFDM symbols
        let num_payload_syms = payload_symbols.len().div_ceil(data_per_sym);
        for sym_idx in 0..num_payload_syms {
            let start = sym_idx * data_per_sym;
            let end = (start + data_per_sym).min(payload_symbols.len());
            let mut data = payload_symbols[start..end].to_vec();
            data.resize(data_per_sym, Complex32::new(1.0, 0.0));
            let global_sym = num_header_syms + sym_idx;
            let carriers = self.pilots.insert_pilots(&data, global_sym);
            samples.extend(self.build_ofdm_symbol(&carriers));
        }

        // 5. PAPR clipping
        papr_clip(&samples, papr_target_db)
    }

    /// Demodulate audio samples back into a frame header and payload.
    ///
    /// Returns `None` if synchronization or header parsing fails.
    pub fn demodulate(&self, samples: &[f32]) -> Option<(CoppaHeader, Vec<u8>)> {
        let symbol_len = self.profile.fft_size + self.profile.cp_samples;
        let data_per_sym = self.data_carriers_per_symbol();
        let total_active = self.profile.total_active_carriers();

        // 1. Detect preamble version
        let (_version, timing_offset) = detect_coppa_version(samples, &self.profile)?;

        // 2. Skip past preamble: 2 sync symbols + 1 fine sync = 3 symbols
        let data_start = timing_offset + 3 * symbol_len;
        if data_start >= samples.len() {
            return None;
        }

        // 3. Demodulate header symbols (2 symbols -> 88+ bits, take first 48)
        let num_header_syms = 48usize.div_ceil(data_per_sym);
        let mut header_bits = Vec::with_capacity(48);

        for sym_idx in 0..num_header_syms {
            let sym_start = data_start + sym_idx * symbol_len;
            if sym_start + symbol_len > samples.len() {
                return None;
            }

            let sym_samples = &samples[sym_start..sym_start + symbol_len];
            let carriers = self.demod_ofdm_symbol(sym_samples);

            // Channel estimation from pilots
            let pilot_info = self.extract_pilot_info(&carriers, sym_idx);
            let equalized = self.equalize_carriers(&carriers, &pilot_info, total_active);

            // Extract data carriers and BPSK hard decision
            let data = self.pilots.extract_data(&equalized, sym_idx);
            for &val in &data {
                if header_bits.len() < 48 {
                    header_bits.push(if val.re >= 0.0 { 0u8 } else { 1u8 });
                }
            }
        }

        // 4. Parse header
        let header = CoppaHeader::from_bits(&header_bits)?;

        // 5. Demodulate payload symbols
        let total_payload_bits = header.payload_len as usize * 8;
        let num_payload_syms = total_payload_bits.div_ceil(data_per_sym);
        let mut payload_bits = Vec::with_capacity(total_payload_bits);

        for sym_idx in 0..num_payload_syms {
            let global_sym = num_header_syms + sym_idx;
            let sym_start = data_start + global_sym * symbol_len;
            if sym_start + symbol_len > samples.len() {
                break;
            }

            let sym_samples = &samples[sym_start..sym_start + symbol_len];
            let carriers = self.demod_ofdm_symbol(sym_samples);

            let pilot_info = self.extract_pilot_info(&carriers, global_sym);
            let equalized = self.equalize_carriers(&carriers, &pilot_info, total_active);

            let data = self.pilots.extract_data(&equalized, global_sym);
            for &val in &data {
                if payload_bits.len() < total_payload_bits {
                    payload_bits.push(if val.re >= 0.0 { 0u8 } else { 1u8 });
                }
            }
        }

        // 6. Convert payload bits to bytes (MSB first)
        let num_bytes = payload_bits.len() / 8;
        let mut payload = Vec::with_capacity(num_bytes);
        for chunk in payload_bits.chunks(8) {
            if chunk.len() == 8 {
                let mut byte = 0u8;
                for (i, &bit) in chunk.iter().enumerate() {
                    byte |= (bit & 1) << (7 - i);
                }
                payload.push(byte);
            }
        }

        Some((header, payload))
    }

    /// Demodulate returning equalized complex symbols and per-carrier noise variance.
    ///
    /// Returns `(header, payload_symbols, noise_variances)` where:
    /// - `payload_symbols`: equalized Complex32 values, one per data carrier per OFDM symbol
    /// - `noise_variances`: per-symbol effective noise variance (one per payload symbol)
    ///
    /// Returns `None` if preamble sync or header parsing fails.
    pub fn demodulate_soft(
        &self,
        samples: &[f32],
    ) -> Option<(CoppaHeader, Vec<Complex32>, Vec<f32>)> {
        let symbol_len = self.profile.fft_size + self.profile.cp_samples;
        let data_per_sym = self.data_carriers_per_symbol();
        let total_active = self.profile.total_active_carriers();

        // 1. Sync
        let (_version, timing_offset) = detect_coppa_version(samples, &self.profile)?;
        let data_start = timing_offset + 3 * symbol_len;
        if data_start >= samples.len() {
            return None;
        }

        // 2. Header (hard-decision BPSK, same as demodulate)
        let num_header_syms = 48usize.div_ceil(data_per_sym);
        let mut header_bits = Vec::with_capacity(48);

        for sym_idx in 0..num_header_syms {
            let sym_start = data_start + sym_idx * symbol_len;
            if sym_start + symbol_len > samples.len() {
                return None;
            }
            let sym_samples = &samples[sym_start..sym_start + symbol_len];
            let carriers = self.demod_ofdm_symbol(sym_samples);
            let pilot_info = self.extract_pilot_info(&carriers, sym_idx);
            let equalized = self.equalize_carriers(&carriers, &pilot_info, total_active);
            let data = self.pilots.extract_data(&equalized, sym_idx);
            for &val in &data {
                if header_bits.len() < 48 {
                    header_bits.push(if val.re >= 0.0 { 0u8 } else { 1u8 });
                }
            }
        }

        let header = CoppaHeader::from_bits(&header_bits)?;

        // 3. Payload: return equalized symbols + per-carrier noise
        let total_payload_bits = header.payload_len as usize * 8;
        let num_payload_syms = total_payload_bits.div_ceil(data_per_sym);
        let mut payload_symbols = Vec::new();
        let mut noise_variances = Vec::new();

        for sym_idx in 0..num_payload_syms {
            let global_sym = num_header_syms + sym_idx;
            let sym_start = data_start + global_sym * symbol_len;
            if sym_start + symbol_len > samples.len() {
                break;
            }
            let sym_samples = &samples[sym_start..sym_start + symbol_len];
            let carriers = self.demod_ofdm_symbol(sym_samples);
            let pilot_info = self.extract_pilot_info(&carriers, global_sym);

            // Channel estimation with noise extraction
            let mut estimator = LinearInterpolationEstimator::new(total_active);
            estimator.update(&pilot_info);
            let equalized = mmse_equalize(&carriers, &estimator, total_active);

            // Extract data carriers and their per-carrier noise
            let data = self.pilots.extract_data(&equalized, global_sym);
            let data_indices = self.pilots.data_indices(global_sym);
            let carrier_noise = estimator.per_carrier_noise(&data_indices);

            payload_symbols.extend_from_slice(&data);
            noise_variances.extend_from_slice(&carrier_noise);
        }

        Some((header, payload_symbols, noise_variances))
    }

    /// Soft-demodulate a received frame, computing the coded payload symbol
    /// count internally from the header's speed level.
    ///
    /// After decoding the header, the method looks up `bits_per_symbol` from
    /// `SPEED_LEVELS` and derives the exact number of constellation symbols
    /// needed for one LDPC codeword (1944 coded bits). This means higher-order
    /// modulations (e.g. 64-QAM) demodulate far fewer OFDM symbols than BPSK.
    pub fn demodulate_soft_coded(
        &self,
        samples: &[f32],
    ) -> Option<(CoppaHeader, Vec<Complex32>, Vec<f32>)> {
        let symbol_len = self.profile.fft_size + self.profile.cp_samples;
        let data_per_sym = self.data_carriers_per_symbol();
        let total_active = self.profile.total_active_carriers();

        // 1. Sync
        let (_version, timing_offset) = detect_coppa_version(samples, &self.profile)?;
        let data_start = timing_offset + 3 * symbol_len;
        if data_start >= samples.len() {
            return None;
        }

        // 2. Header (hard-decision BPSK)
        let num_header_syms = 48usize.div_ceil(data_per_sym);
        let mut header_bits = Vec::with_capacity(48);

        for sym_idx in 0..num_header_syms {
            let sym_start = data_start + sym_idx * symbol_len;
            if sym_start + symbol_len > samples.len() {
                return None;
            }
            let sym_samples = &samples[sym_start..sym_start + symbol_len];
            let carriers = self.demod_ofdm_symbol(sym_samples);
            let pilot_info = self.extract_pilot_info(&carriers, sym_idx);
            let equalized = self.equalize_carriers(&carriers, &pilot_info, total_active);
            let data = self.pilots.extract_data(&equalized, sym_idx);
            for &val in &data {
                if header_bits.len() < 48 {
                    header_bits.push(if val.re >= 0.0 { 0u8 } else { 1u8 });
                }
            }
        }

        let header = CoppaHeader::from_bits(&header_bits)?;

        // Compute coded payload symbols from header's speed level.
        // 1944 = LDPC coded block length (Z=81, 24 base columns).
        const CODED_BLOCK_LEN: usize = 1944;
        let coded_symbols = SPEED_LEVELS
            .iter()
            .find(|s| s.level == header.speed_level)
            .map(|s| CODED_BLOCK_LEN.div_ceil(s.bits_per_symbol as usize))
            .unwrap_or(CODED_BLOCK_LEN);

        // 3. Payload: demodulate enough OFDM symbols for `coded_symbols` complex values
        let num_payload_syms = coded_symbols.div_ceil(data_per_sym);
        let mut payload_symbols = Vec::new();
        let mut noise_variances = Vec::new();

        for sym_idx in 0..num_payload_syms {
            let global_sym = num_header_syms + sym_idx;
            let sym_start = data_start + global_sym * symbol_len;
            if sym_start + symbol_len > samples.len() {
                break;
            }
            let sym_samples = &samples[sym_start..sym_start + symbol_len];
            let carriers = self.demod_ofdm_symbol(sym_samples);
            let pilot_info = self.extract_pilot_info(&carriers, global_sym);

            let mut estimator = LinearInterpolationEstimator::new(total_active);
            estimator.update(&pilot_info);
            let equalized = mmse_equalize(&carriers, &estimator, total_active);

            let data = self.pilots.extract_data(&equalized, global_sym);
            let data_indices = self.pilots.data_indices(global_sym);
            let carrier_noise = estimator.per_carrier_noise(&data_indices);

            payload_symbols.extend_from_slice(&data);
            noise_variances.extend_from_slice(&carrier_noise);
        }

        Some((header, payload_symbols, noise_variances))
    }

    /// Extract pilot info tuples from demodulated carriers for a given symbol number.
    fn extract_pilot_info(
        &self,
        carriers: &[Complex32],
        symbol_num: usize,
    ) -> Vec<(usize, Complex32, Complex32)> {
        self.pilots
            .extract_pilots(carriers, symbol_num)
            .iter()
            .map(|&(idx, received)| (idx, received, Complex32::new(1.0, 0.0)))
            .collect()
    }

    /// Run channel estimation and MMSE equalization on carriers.
    fn equalize_carriers(
        &self,
        carriers: &[Complex32],
        pilot_info: &[(usize, Complex32, Complex32)],
        num_carriers: usize,
    ) -> Vec<Complex32> {
        let mut estimator = LinearInterpolationEstimator::new(num_carriers);
        estimator.update(pilot_info);
        mmse_equalize(carriers, &estimator, num_carriers)
    }
}

#[cfg(test)]
mod tests {
    use super::super::frame::CoppaFrameType;
    use super::*;

    #[test]
    fn test_coppa_modem_modulate_produces_audio() {
        let profile = CoppaProfile::hf_standard();
        let modem = CoppaModem::new(profile, 1);

        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: 100,
        };
        let payload = vec![0xABu8; 100];

        let samples = modem.modulate(&header, &payload);

        assert!(!samples.is_empty(), "Output should be non-empty");
        assert!(
            samples.len() > 5000,
            "Output should be longer than 5000 samples, got {}",
            samples.len()
        );
        for (i, &s) in samples.iter().enumerate() {
            assert!(s.is_finite(), "Sample {} should be finite, got {}", i, s);
        }
    }

    #[test]
    fn test_speed_levels_no_32qam() {
        assert_eq!(SPEED_LEVELS.len(), 9);

        // Level 1: BPSK 1/4
        assert_eq!(SPEED_LEVELS[0].level, 1);
        assert_eq!(SPEED_LEVELS[0].bits_per_symbol, 1);
        assert_eq!(SPEED_LEVELS[0].ldpc_rate_num, 1);
        assert_eq!(SPEED_LEVELS[0].ldpc_rate_den, 4);

        // Level 7: 16QAM 3/4 (last before gap)
        assert_eq!(SPEED_LEVELS[6].level, 7);
        assert_eq!(SPEED_LEVELS[6].bits_per_symbol, 4);

        // Level 9 on wire → index 7 in array
        assert_eq!(SPEED_LEVELS[7].level, 9);
        assert_eq!(SPEED_LEVELS[7].bits_per_symbol, 6);
        assert_eq!(SPEED_LEVELS[7].ldpc_rate_num, 2);
        assert_eq!(SPEED_LEVELS[7].ldpc_rate_den, 3);

        // Level 10 on wire → index 8 in array
        assert_eq!(SPEED_LEVELS[8].level, 10);
        assert_eq!(SPEED_LEVELS[8].bits_per_symbol, 6);
        assert_eq!(SPEED_LEVELS[8].ldpc_rate_num, 7);
        assert_eq!(SPEED_LEVELS[8].ldpc_rate_den, 8);
    }

    #[test]
    fn test_coppa_modem_clean_loopback() {
        let profile = CoppaProfile::hf_standard();
        let modem = CoppaModem::new(profile, 1);

        let payload = b"Hello, Coppa Protocol!";
        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 7,
            payload_len: payload.len() as u16,
        };

        let samples = modem.modulate(&header, payload);

        let (rx_header, rx_payload) = modem
            .demodulate(&samples)
            .expect("Demodulation should succeed");

        assert_eq!(rx_header.version, header.version);
        assert_eq!(rx_header.phy_mode, header.phy_mode);
        assert_eq!(rx_header.frame_type, header.frame_type);
        assert_eq!(rx_header.bandwidth, header.bandwidth);
        assert_eq!(rx_header.fec_type, header.fec_type);
        assert_eq!(rx_header.speed_level, header.speed_level);
        assert_eq!(rx_header.seq_num, header.seq_num);
        assert_eq!(rx_header.payload_len, header.payload_len);
        assert_eq!(rx_payload, payload, "Payload should match exactly");
    }

    #[test]
    fn test_modulate_mapped_produces_audio() {
        let profile = CoppaProfile::hf_standard();
        let modem = CoppaModem::new(profile, 1);

        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 3,
            seq_num: 0,
            payload_len: 50,
        };

        let payload_symbols: Vec<Complex32> = (0..100)
            .map(|i| {
                let angle = (i as f32) * 0.5;
                Complex32::new(angle.cos(), angle.sin()) * std::f32::consts::FRAC_1_SQRT_2
            })
            .collect();

        let samples = modem.modulate_mapped(&header, &payload_symbols, 7.0);
        assert!(!samples.is_empty());
        for (i, &s) in samples.iter().enumerate() {
            assert!(s.is_finite(), "Sample {} is not finite: {}", i, s);
        }
    }

    #[test]
    fn test_demodulate_soft_returns_symbols_and_noise() {
        let profile = CoppaProfile::hf_standard();
        let modem = CoppaModem::new(profile, 1);

        let header = CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level: 2,
            seq_num: 0,
            payload_len: 20,
        };
        let payload = vec![0xABu8; 20];
        let samples = modem.modulate(&header, &payload);

        let (rx_header, symbols, noise_vars) = modem
            .demodulate_soft(&samples)
            .expect("demodulate_soft should succeed");

        assert_eq!(rx_header.version, header.version);
        assert_eq!(rx_header.payload_len, header.payload_len);
        assert!(!symbols.is_empty(), "Should return payload symbols");
        assert!(!noise_vars.is_empty(), "Should return noise variances");
        assert_eq!(
            noise_vars.len(),
            symbols.len(),
            "One noise variance per symbol"
        );
        for &nv in &noise_vars {
            assert!(nv > 0.0, "Noise variance should be positive");
        }
    }

    #[test]
    fn test_speed_levels_have_papr_targets() {
        use super::SPEED_LEVELS;
        // BPSK levels should have lower PAPR targets than 64-QAM levels
        let bpsk = SPEED_LEVELS.iter().find(|s| s.level == 1).unwrap();
        let qam64 = SPEED_LEVELS.iter().find(|s| s.level == 9).unwrap();
        assert!(
            bpsk.papr_target_db < qam64.papr_target_db,
            "BPSK target ({}) should be less than 64-QAM target ({})",
            bpsk.papr_target_db,
            qam64.papr_target_db
        );

        // Verify specific values
        assert_eq!(bpsk.papr_target_db, 6.0);
        assert_eq!(qam64.papr_target_db, 11.0);
    }
}
