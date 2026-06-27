//! Rate-1/2, constraint length 7 convolutional code with Viterbi decoder.
//!
//! Generators: G1 = 0o171 (0x79), G2 = 0o133 (0x5B) — the NASA/CCSDS standard.
use coppa_codec::traits::FecCodec;

const CONSTRAINT_LENGTH: usize = 7;
const NUM_STATES: usize = 1 << (CONSTRAINT_LENGTH - 1); // 64
const G1: u8 = 0x79; // 0o171
const G2: u8 = 0x5B; // 0o133

/// Rate-1/2 convolutional encoder.
pub struct ConvEncoder {
    state: u8,
}

impl Default for ConvEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ConvEncoder {
    pub fn new() -> Self {
        Self { state: 0 }
    }

    fn encode_bit(&mut self, bit: u8) -> (u8, u8) {
        let full_reg = ((bit & 1) << 6) | self.state;
        let c1 = (full_reg & G1).count_ones() as u8 % 2;
        let c2 = (full_reg & G2).count_ones() as u8 % 2;
        self.state = ((bit & 1) << 5) | (self.state >> 1);
        (c1, c2)
    }
}

impl FecCodec for ConvEncoder {
    fn rate(&self) -> f32 {
        0.5
    }

    fn encode(&mut self, bits: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity((bits.len() + CONSTRAINT_LENGTH - 1) * 2);

        for &bit in bits {
            let (c1, c2) = self.encode_bit(bit);
            output.push(c1);
            output.push(c2);
        }

        // Flush with K-1 zero bits to terminate the trellis
        for _ in 0..(CONSTRAINT_LENGTH - 1) {
            let (c1, c2) = self.encode_bit(0);
            output.push(c1);
            output.push(c2);
        }

        self.state = 0;
        output
    }

    fn decode(&self, soft_symbols: &[f32]) -> Vec<u8> {
        let decoder = ViterbiDecoder::new();
        decoder.decode(soft_symbols)
    }
}

/// Soft-decision Viterbi decoder for rate-1/2, K=7 convolutional code.
///
/// ## LLR / Soft Symbol Sign Convention
///
/// The decoder expects soft symbols where:
/// - **Positive value** indicates bit 0 (maps to BPSK symbol +1.0)
/// - **Negative value** indicates bit 1 (maps to BPSK symbol -1.0)
///
/// This matches the standard BPSK convention used by `BpskMapper`:
/// bit 0 -> +1.0, bit 1 -> -1.0. The magnitude of the soft symbol
/// represents confidence (larger magnitude = more reliable decision).
/// Branch metrics are computed as Euclidean distance between the received
/// soft symbol and the expected +1/-1 reference.
pub struct ViterbiDecoder {
    /// Precomputed state transitions.
    transitions: Vec<[(u8, u8, usize); 2]>,
}

impl Default for ViterbiDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ViterbiDecoder {
    pub fn new() -> Self {
        // Precompute transitions once
        let mut transitions = vec![[(0u8, 0u8, 0usize); 2]; NUM_STATES];
        for (state, row) in transitions.iter_mut().enumerate() {
            for input_bit in 0..2u8 {
                let full_reg = ((input_bit & 1) << 6) | state as u8;
                let c1 = (full_reg & G1).count_ones() as u8 % 2;
                let c2 = (full_reg & G2).count_ones() as u8 % 2;
                let next_state = ((input_bit & 1) as usize) << 5 | (state >> 1);
                row[input_bit as usize] = (c1, c2, next_state);
            }
        }

        Self { transitions }
    }

    pub fn decode(&self, soft_symbols: &[f32]) -> Vec<u8> {
        if soft_symbols.len() < 2 {
            return Vec::new();
        }

        let num_pairs = soft_symbols.len() / 2;
        let num_data_bits = num_pairs.saturating_sub(CONSTRAINT_LENGTH - 1);
        if num_data_bits == 0 {
            return Vec::new();
        }

        let mut prev_metrics = vec![f32::MAX; NUM_STATES];
        let mut curr_metrics = vec![f32::MAX; NUM_STATES];
        prev_metrics[0] = 0.0;

        let mut survivors = vec![vec![0u8; NUM_STATES]; num_pairs];

        for step in 0..num_pairs {
            let s0 = soft_symbols[step * 2];
            let s1 = soft_symbols[step * 2 + 1];

            for m in curr_metrics.iter_mut() {
                *m = f32::MAX;
            }

            for (state, &prev_m) in prev_metrics.iter().enumerate() {
                if prev_m == f32::MAX {
                    continue;
                }

                for input_bit in 0..2u8 {
                    let (c1, c2, next_state) = self.transitions[state][input_bit as usize];

                    let exp1 = if c1 == 0 { 1.0f32 } else { -1.0 };
                    let exp2 = if c2 == 0 { 1.0f32 } else { -1.0 };
                    let branch_metric = (s0 - exp1).powi(2) + (s1 - exp2).powi(2);

                    let total = prev_m + branch_metric;

                    if total < curr_metrics[next_state] {
                        curr_metrics[next_state] = total;
                        survivors[step][next_state] = state as u8;
                    }
                }
            }

            std::mem::swap(&mut prev_metrics, &mut curr_metrics);
        }

        // Traceback from state 0 (trellis terminated by K-1 flush bits).
        //
        // Input bit recovery: the encoder uses `state = (bit << 5) | (state >> 1)`,
        // meaning the input bit enters at position 5 (MSB of the 6-bit state).
        // After the transition, `(next_state >> 5) & 1` recovers the bit that
        // was shifted in. This invariant depends on the encoder's shift direction.
        let mut state = 0usize;
        let mut decoded = vec![0u8; num_pairs];

        for step in (0..num_pairs).rev() {
            let prev_state = survivors[step][state] as usize;
            let input_bit = ((state >> 5) & 1) as u8;
            decoded[step] = input_bit;
            state = prev_state;
        }

        decoded.truncate(num_data_bits);
        decoded
    }

    pub fn decode_hard(&self, bits: &[u8]) -> Vec<u8> {
        let soft: Vec<f32> = bits
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        self.decode(&soft)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_output_length() {
        let mut enc = ConvEncoder::new();
        let input = vec![1, 0, 1, 1, 0];
        let output = enc.encode(&input);
        assert_eq!(output.len(), (input.len() + CONSTRAINT_LENGTH - 1) * 2);
    }

    #[test]
    fn test_encoder_decoder_roundtrip() {
        let mut enc = ConvEncoder::new();
        let dec = ViterbiDecoder::new();

        let input = vec![1, 0, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 1, 1, 0, 1];
        let encoded = enc.encode(&input);
        let decoded = dec.decode_hard(&encoded);

        assert_eq!(decoded, input);
    }

    #[test]
    fn test_viterbi_corrects_errors() {
        let mut enc = ConvEncoder::new();
        let dec = ViterbiDecoder::new();

        let input = vec![1, 0, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 1, 1, 0, 1, 0, 0, 1, 1];
        let mut encoded = enc.encode(&input);

        let error_positions = vec![3, 10, 19, 28];
        for &pos in &error_positions {
            if pos < encoded.len() {
                encoded[pos] ^= 1;
            }
        }

        let decoded = dec.decode_hard(&encoded);
        assert_eq!(decoded, input, "Viterbi should correct sparse bit errors");
    }

    #[test]
    fn viterbi_decode_is_scale_invariant() {
        // Audit guard: unlike the LDPC offset-min-sum decoder (whose fixed additive offset broke
        // scale-invariance and discarded faded frames), the Viterbi Euclidean branch metric
        // (s - ±1)^2 is scale-invariant — scaling all soft inputs by c>0 scales every competing
        // path-metric difference by c, preserving the argmin. Decoding the SAME soft symbols at
        // unit, tiny, and large scale must give the identical bits.
        let mut enc = ConvEncoder::new();
        let dec = ViterbiDecoder::new();
        let input: Vec<u8> = vec![1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 0];
        let encoded = enc.encode(&input);
        let soft: Vec<f32> = encoded
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                let base = if b == 0 { 1.0 } else { -1.0 };
                base + 0.3 * ((i as f32 * 1.7).sin())
            })
            .collect();

        let unit = dec.decode(&soft);
        assert_eq!(unit, input, "baseline soft decode must be correct");
        for scale in [0.01f32, 0.1, 10.0, 100.0] {
            let scaled: Vec<f32> = soft.iter().map(|s| s * scale).collect();
            assert_eq!(
                dec.decode(&scaled),
                unit,
                "Viterbi must be scale-invariant (scale={scale})"
            );
        }
    }

    #[test]
    fn test_viterbi_soft_decision() {
        let mut enc = ConvEncoder::new();
        let dec = ViterbiDecoder::new();

        let input: Vec<u8> = vec![1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 0];
        let encoded = enc.encode(&input);

        let soft: Vec<f32> = encoded
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                let base = if b == 0 { 1.0 } else { -1.0 };
                let noise = 0.3 * ((i as f32 * 1.7).sin());
                base + noise
            })
            .collect();

        let decoded = dec.decode(&soft);
        assert_eq!(decoded, input, "Soft Viterbi should handle moderate noise");
    }

    #[test]
    fn test_fec_codec_trait() {
        let mut enc = ConvEncoder::new();
        assert!((enc.rate() - 0.5).abs() < 0.01);

        let input = vec![1, 0, 1, 0, 1, 1, 0, 0];
        let encoded = FecCodec::encode(&mut enc, &input);
        let decoded = FecCodec::decode(&enc, &{
            let soft: Vec<f32> = encoded
                .iter()
                .map(|&b| if b == 0 { 1.0 } else { -1.0 })
                .collect();
            soft
        });
        assert_eq!(decoded, input);
    }

    #[test]
    fn test_encoder_deterministic() {
        let input = vec![1, 0, 1, 0];
        let mut enc1 = ConvEncoder::new();
        let mut enc2 = ConvEncoder::new();
        assert_eq!(enc1.encode(&input), enc2.encode(&input));
    }

    #[test]
    fn test_empty_input() {
        let mut enc = ConvEncoder::new();
        let dec = ViterbiDecoder::new();

        let encoded = enc.encode(&[]);
        assert_eq!(encoded.len(), (CONSTRAINT_LENGTH - 1) * 2);

        let decoded = dec.decode_hard(&encoded);
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_various_patterns() {
        let dec = ViterbiDecoder::new();

        for pattern in &[
            vec![0u8; 20],
            vec![1u8; 20],
            vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1],
            vec![1, 1, 0, 0, 1, 1, 0, 0, 1, 1],
        ] {
            let mut enc = ConvEncoder::new();
            let encoded = enc.encode(pattern);
            let decoded = dec.decode_hard(&encoded);
            assert_eq!(&decoded, pattern, "Failed for pattern {:?}", pattern);
        }
    }
}
