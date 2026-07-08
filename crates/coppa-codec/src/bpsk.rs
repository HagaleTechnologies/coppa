//! BPSK (Binary Phase Shift Keying) modem and constellation mapper.
use anyhow::Result;
use num_complex::Complex32;
use std::f32::consts::PI;

use coppa_dsp::agc::AdaptiveAgc;
use coppa_dsp::carrier_recovery::CostasLoop;
use coppa_dsp::filter::{NormMode, RrcFilter};
use coppa_dsp::timing_recovery::GardnerTimingRecovery;

use crate::traits::{ConstellationMapper, Modem};

/// BPSK constellation mapper.
///
/// Bit `0` maps to `+1` and bit `1` maps to `-1`, so a hard demap of the
/// mapped symbol recovers the original bit:
///
/// ```
/// use coppa_codec::bpsk::BpskMapper;
/// use coppa_codec::traits::ConstellationMapper;
///
/// let mapper = BpskMapper;
/// for bit in [0u8, 1] {
///     let symbol = mapper.map(&[bit]);
///     assert_eq!(mapper.demap_hard(symbol), vec![bit]);
/// }
/// ```
pub struct BpskMapper;

impl ConstellationMapper for BpskMapper {
    fn bits_per_symbol(&self) -> usize {
        1
    }

    fn map(&self, bits: &[u8]) -> Complex32 {
        if bits.is_empty() || bits[0] == 0 {
            Complex32::new(1.0, 0.0)
        } else {
            Complex32::new(-1.0, 0.0)
        }
    }

    fn demap_hard(&self, symbol: Complex32) -> Vec<u8> {
        vec![if symbol.re >= 0.0 { 0 } else { 1 }]
    }

    fn demap_soft(&self, symbol: Complex32, noise_variance: f32) -> Vec<f32> {
        let nv = if noise_variance > 1e-10 {
            noise_variance
        } else {
            1e-10
        };
        // Exact max-log BPSK LLR = 4 * re / sigma^2 (positive means more likely 0).
        // Matches `push_header_llrs` in `coppa_codec::ofdm::coppa_modem` (the
        // protected-header path), which already uses this exact scale -- see
        // Task 2's review finding: this mapper previously used `2 * re / sigma^2`,
        // a factor-of-2 mismatch against the header path.
        vec![4.0 * symbol.re / nv]
    }
}

/// Full BPSK modem with DSP chain.
///
/// TX: bits -> differential encode -> NRZ baseband -> DC-gain RRC pulse shape -> upconvert to carrier
/// RX: AGC -> Costas loop -> Energy-normalized RRC matched filter -> Gardner timing -> soft differential decode -> soft symbols
///
/// Differential encoding at the symbol level (`d[n] = b[n] XOR d[n-1]`) makes
/// the modem immune to 180-degree Costas loop phase ambiguity. A reference
/// symbol is prepended on TX so that after differential decoding (which produces
/// N-1 outputs from N symbols), the RX bit stream has the same length as the input.
///
/// ```
/// use coppa_codec::bpsk::BpskModem;
/// use coppa_codec::traits::Modem;
///
/// let mut modem = BpskModem::new();
/// let bits = vec![0u8, 1, 0, 1];
///
/// // TX prepends one reference symbol, so the audio is (N + 1) * sps samples.
/// let audio = modem.modulate(&bits).unwrap();
/// assert_eq!(audio.len(), (bits.len() + 1) * modem.samples_per_symbol());
///
/// // RX produces soft symbols (sign = bit decision, magnitude = confidence).
/// let soft = modem.demodulate_soft(&audio).unwrap();
/// assert!(!soft.is_empty());
/// ```
pub struct BpskModem {
    sample_rate_hz: f32,
    carrier_freq: f32,
    sps: usize,
    rrc_tx: RrcFilter,
    rrc_rx: RrcFilter,
    agc: AdaptiveAgc,
    costas: CostasLoop,
    timing: GardnerTimingRecovery,
}

impl Default for BpskModem {
    fn default() -> Self {
        Self::new()
    }
}

impl BpskModem {
    pub fn new() -> Self {
        Self::with_params(8000.0, 31.25, 1000.0)
    }

    /// Create a BPSK modem with configurable parameters.
    pub fn with_params(sample_rate: f32, symbol_rate: f32, carrier_freq: f32) -> Self {
        let sps = (sample_rate / symbol_rate) as usize;
        let rrc_tx = RrcFilter::new_with_norm(0.35, 4, sps, NormMode::DcGain);
        let rrc_rx = RrcFilter::new_with_norm(0.35, 4, sps, NormMode::Energy);
        let agc = AdaptiveAgc::new(1.0, sps);
        let costas = CostasLoop::new(carrier_freq, sample_rate, 0.01);
        let timing = GardnerTimingRecovery::new(sps as f32, 0.01);

        Self {
            sample_rate_hz: sample_rate,
            carrier_freq,
            sps,
            rrc_tx,
            rrc_rx,
            agc,
            costas,
            timing,
        }
    }

    /// Reset all DSP state for a fresh demodulation pass.
    pub fn reset(&mut self) {
        self.agc.reset();
        self.costas.reset();
        self.timing.reset();
    }
}

impl Modem for BpskModem {
    fn modulate(&self, bits: &[u8]) -> Result<Vec<f32>> {
        // Differential encoding: d[n] = b[n] XOR d[n-1]
        // This resolves 180-degree Costas loop phase ambiguity at the modem level.
        // Prepend a reference symbol (0 → +1) so that after soft differential
        // decoding (which produces N-1 outputs from N inputs), the RX bit
        // stream has the same length as the original input.
        let mut diff_bits = Vec::with_capacity(bits.len() + 1);
        let mut prev = 0u8;
        diff_bits.push(prev); // reference symbol
        for &bit in bits {
            prev ^= bit;
            diff_bits.push(prev);
        }

        // NRZ baseband: each differentially-encoded symbol is held for sps samples.
        // The DC-gain RRC filter shapes the spectrum.
        // NOTE: Impulse-train TX (A1) is theoretically superior for zero-ISI at
        // sampling instants (RRC*RRC = RC), but the extreme amplitude reduction
        // (1/sps factor) causes Costas lock failure at high sps values like 256.
        // NRZ is retained for reliable operation across all symbol rates.
        let mut baseband = Vec::with_capacity(diff_bits.len() * self.sps);
        for &bit in &diff_bits {
            let val = if bit == 0 { 1.0f32 } else { -1.0 };
            for _ in 0..self.sps {
                baseband.push(val);
            }
        }

        let shaped = self.rrc_tx.filter(&baseband);

        let mut samples = Vec::with_capacity(shaped.len());
        for (n, &s) in shaped.iter().enumerate() {
            let t = n as f32 / self.sample_rate_hz;
            let carrier = (2.0 * PI * self.carrier_freq * t).cos();
            samples.push(s * carrier);
        }

        Ok(samples)
    }

    fn demodulate_soft(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        let normalized = self.agc.process(samples);
        let baseband = self.costas.process(&normalized);
        let filtered = self.rrc_rx.filter(&baseband);
        let symbols = self.timing.recover(&filtered);

        // Soft differential decoding: multiply consecutive symbols.
        // s[n] * s[n-1] recovers the original data polarity regardless of
        // 180-degree Costas ambiguity. Normalize by the geometric mean of
        // the absolute values to preserve soft magnitude.
        if symbols.len() < 2 {
            return Ok(Vec::new());
        }
        let mut decoded = Vec::with_capacity(symbols.len() - 1);
        for i in 1..symbols.len() {
            let product = symbols[i] * symbols[i - 1];
            let magnitude = (symbols[i].abs() * symbols[i - 1].abs()).sqrt().max(1e-10);
            decoded.push(product / magnitude);
        }

        Ok(decoded)
    }

    fn sample_rate(&self) -> f32 {
        self.sample_rate_hz
    }

    fn samples_per_symbol(&self) -> usize {
        self.sps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bpsk_mapper_roundtrip() {
        let mapper = BpskMapper;
        for bit in [0u8, 1] {
            let sym = mapper.map(&[bit]);
            let demapped = mapper.demap_hard(sym);
            assert_eq!(demapped[0], bit);
        }
    }

    #[test]
    fn test_bpsk_soft_demap() {
        let mapper = BpskMapper;
        let llr = mapper.demap_soft(Complex32::new(0.5, 0.0), 1.0);
        assert!(llr[0] > 0.0, "Positive re should give positive LLR (bit 0)");
        assert!(
            (llr[0] - 2.0).abs() < 0.01,
            "LLR should be ~4*0.5/1.0 = 2.0, got {}",
            llr[0]
        );

        let llr = mapper.demap_soft(Complex32::new(-0.5, 0.0), 1.0);
        assert!(llr[0] < 0.0, "Negative re should give negative LLR (bit 1)");
    }

    /// Exact max-log BPSK LLR scale (decision 8): `4 * re / sigma^2`. Matches
    /// `push_header_llrs`'s scale in `coppa_codec::ofdm::coppa_modem` exactly, so the
    /// header and payload paths compute LLRs on the same footing.
    #[test]
    fn test_bpsk_soft_demap_exact_max_log_scale() {
        let mapper = BpskMapper;
        let llr = mapper.demap_soft(Complex32::new(1.0, 0.0), 0.5);
        assert!(
            (llr[0] - 8.0).abs() < 1e-4,
            "LLR should be exactly 4*1.0/0.5 = 8.0, got {}",
            llr[0]
        );
    }

    #[test]
    fn test_bpsk_modulation() {
        let modem = BpskModem::new();
        let bits = vec![0, 1, 0, 1];
        let samples = modem.modulate(&bits).unwrap();
        // NRZ TX with reference symbol: output length is (N+1) * sps
        assert_eq!(samples.len(), (bits.len() + 1) * modem.samples_per_symbol());
    }

    #[test]
    fn test_bpsk_loopback() {
        let mut modem = BpskModem::new();
        // Use a longer sequence so the threshold is meaningful after settling.
        // Differential encoding + decoding means we lose one symbol at the start.
        let original_bits: Vec<u8> = (0..60).map(|i| ((i * 7 + 3) % 2) as u8).collect();
        let samples = modem.modulate(&original_bits).unwrap();
        let soft = modem.demodulate_soft(&samples).unwrap();
        let decoded_bits: Vec<u8> = soft.iter().map(|&s| if s >= 0.0 { 0 } else { 1 }).collect();

        // Differential decoding loses one symbol, timing recovery may lose a few more
        assert!(
            decoded_bits.len() >= original_bits.len() - 6,
            "Should recover most symbols, got {} of {}",
            decoded_bits.len(),
            original_bits.len()
        );

        // Verify bit correctness after Costas loop settling.
        // With differential encoding + impulse-train TX, the recovered bit
        // stream may be offset by a few symbols due to timing recovery alignment
        // and the differential decode losing one symbol.
        // Try all plausible offsets and verify best alignment.
        let settle = 4;
        let mut best_correct = 0;
        let mut best_total = 1;
        for offset in 0..6 {
            let mut correct = 0;
            let mut total = 0;
            for (i, &bit) in decoded_bits.iter().enumerate().skip(settle) {
                let orig_idx = i + offset;
                if orig_idx < original_bits.len() {
                    total += 1;
                    if bit == original_bits[orig_idx] {
                        correct += 1;
                    }
                }
            }
            if total > 0 && correct * best_total > best_correct * total {
                best_correct = correct;
                best_total = total;
            }
        }
        assert!(
            best_total > 0 && best_correct as f32 / best_total as f32 > 0.90,
            "Should decode correctly after settling: {}/{}",
            best_correct,
            best_total
        );
    }

    #[test]
    fn test_bpsk_soft_output() {
        let mut modem = BpskModem::new();
        let bits = vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1];
        let samples = modem.modulate(&bits).unwrap();
        let soft = modem.demodulate_soft(&samples).unwrap();
        // Differential decoding loses 1 symbol, timing may lose a few more
        assert!(soft.len() >= bits.len() - 5);
    }

    #[test]
    fn test_bpsk_with_params() {
        let modem = BpskModem::with_params(48000.0, 300.0, 1500.0);
        assert_eq!(modem.sample_rate(), 48000.0);
        assert_eq!(modem.samples_per_symbol(), 160);
    }
}
