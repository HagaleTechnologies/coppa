//! LDPC (Low-Density Parity-Check) forward error correction.
//!
//! Implements QC-LDPC codes from IEEE Std 802.11-2012, Annex F:
//! - Lifting factor Z = 81
//! - 24-column base matrices
//! - 1,944 coded bits for all rates
//! - Offset min-sum belief propagation decoder with early termination
//!
//! Supported code rates: 1/4, 1/3, 1/2, 2/3, 3/4, 7/8.
pub mod codes;
pub mod decoder;
pub mod encoder;

pub use codes::{CodeRate, LdpcCode};
pub use decoder::LdpcDecoder;
pub use encoder::LdpcEncoder;

use coppa_codec::traits::FecCodec;

/// Complete LDPC codec combining encoder and decoder.
///
/// Implements the `FecCodec` trait for integration with the Coppa DSP chain.
pub struct LdpcCodec {
    encoder: LdpcEncoder,
    decoder: LdpcDecoder,
    code_rate: CodeRate,
}

impl LdpcCodec {
    /// Create a new LDPC codec for the given code rate.
    pub fn new(rate: CodeRate) -> Self {
        let code = LdpcCode::new(rate);
        Self {
            encoder: LdpcEncoder::new(code.clone()),
            decoder: LdpcDecoder::new(code),
            code_rate: rate,
        }
    }

    /// Returns the underlying LDPC code parameters.
    pub fn code(&self) -> &LdpcCode {
        self.decoder.code()
    }

    /// Decode with convergence check. Returns (info_bits, converged).
    pub fn decode_checked(&self, llrs: &[f32]) -> (Vec<u8>, bool) {
        self.decoder.decode_block_checked(llrs)
    }
}

impl FecCodec for LdpcCodec {
    fn rate(&self) -> f32 {
        self.code_rate.as_f32()
    }

    fn encode(&mut self, bits: &[u8]) -> Vec<u8> {
        self.encoder.encode_block(bits)
    }

    fn decode(&self, soft_symbols: &[f32]) -> Vec<u8> {
        self.decoder.decode_block(soft_symbols)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codec_rate_values() {
        let rates = [
            (CodeRate::Rate1_4, 0.25),
            (CodeRate::Rate1_3, 1.0 / 3.0),
            (CodeRate::Rate1_2, 0.5),
            (CodeRate::Rate2_3, 2.0 / 3.0),
            (CodeRate::Rate3_4, 0.75),
            (CodeRate::Rate7_8, 0.875),
        ];
        for (rate, expected) in &rates {
            let codec = LdpcCodec::new(*rate);
            assert!(
                (codec.rate() - expected).abs() < 0.01,
                "Rate {:?} should be {}, got {}",
                rate,
                expected,
                codec.rate()
            );
        }
    }

    #[test]
    fn test_fec_codec_trait_roundtrip() {
        let mut codec = LdpcCodec::new(CodeRate::Rate1_2);
        let info_bits = codec.code().info_bits();

        // All zeros
        let input = vec![0u8; info_bits];
        let encoded = FecCodec::encode(&mut codec, &input);
        assert_eq!(encoded.len(), 1944);

        // Perfect channel (no noise)
        let soft: Vec<f32> = encoded
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        let decoded = FecCodec::decode(&codec, &soft);
        assert_eq!(decoded.len(), info_bits);
        assert_eq!(decoded, input);
    }

    #[test]
    fn test_roundtrip_all_rates() {
        let rates = [
            CodeRate::Rate1_4,
            CodeRate::Rate1_3,
            CodeRate::Rate1_2,
            CodeRate::Rate2_3,
            CodeRate::Rate3_4,
            CodeRate::Rate7_8,
        ];
        for rate in &rates {
            let mut codec = LdpcCodec::new(*rate);
            let info_bits = codec.code().info_bits();

            // Alternating pattern
            let input: Vec<u8> = (0..info_bits).map(|i| (i % 2) as u8).collect();
            let encoded = FecCodec::encode(&mut codec, &input);
            assert_eq!(
                encoded.len(),
                1944,
                "Rate {:?}: coded length should be 1944",
                rate
            );

            let soft: Vec<f32> = encoded
                .iter()
                .map(|&b| if b == 0 { 1.0 } else { -1.0 })
                .collect();
            let decoded = codec.decode(&soft);
            assert_eq!(
                decoded, input,
                "Rate {:?}: roundtrip failed for alternating pattern",
                rate
            );
        }
    }

    #[test]
    fn test_error_correction_rate_1_2() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let mut codec = LdpcCodec::new(CodeRate::Rate1_2);
        let info_bits = codec.code().info_bits();

        let input: Vec<u8> = (0..info_bits).map(|i| ((i * 7 + 3) % 2) as u8).collect();
        let encoded = FecCodec::encode(&mut codec, &input);

        let noise_std = 0.6f32;
        let mut rng = StdRng::seed_from_u64(88);

        let soft: Vec<f32> = encoded
            .iter()
            .map(|&b| {
                let base = if b == 0 { 1.0 } else { -1.0 };
                let u1: f32 = rng.random::<f32>().max(1e-10);
                let u2: f32 = rng.random();
                let noise =
                    noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                base + noise
            })
            .collect();

        let decoded = FecCodec::decode(&codec, &soft);
        assert_eq!(decoded, input, "Should correct moderate noise at rate 1/2");
    }

    #[test]
    fn test_all_zero_data() {
        let mut codec = LdpcCodec::new(CodeRate::Rate1_2);
        let info_bits = codec.code().info_bits();
        let input = vec![0u8; info_bits];
        let encoded = FecCodec::encode(&mut codec, &input);

        let soft: Vec<f32> = encoded
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        let decoded = FecCodec::decode(&codec, &soft);
        assert_eq!(decoded, input, "All-zero data should roundtrip");
    }

    #[test]
    fn test_all_one_data() {
        let mut codec = LdpcCodec::new(CodeRate::Rate1_2);
        let info_bits = codec.code().info_bits();
        let input = vec![1u8; info_bits];
        let encoded = FecCodec::encode(&mut codec, &input);

        let soft: Vec<f32> = encoded
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        let decoded = FecCodec::decode(&codec, &soft);
        assert_eq!(decoded, input, "All-one data should roundtrip");
    }

    #[test]
    fn test_soft_decision_high_confidence() {
        let mut codec = LdpcCodec::new(CodeRate::Rate1_2);
        let info_bits = codec.code().info_bits();
        let input: Vec<u8> = (0..info_bits).map(|i| (i % 2) as u8).collect();
        let encoded = FecCodec::encode(&mut codec, &input);

        // High-confidence soft values (large magnitude)
        let soft: Vec<f32> = encoded
            .iter()
            .map(|&b| if b == 0 { 5.0 } else { -5.0 })
            .collect();
        let decoded = FecCodec::decode(&codec, &soft);
        assert_eq!(
            decoded, input,
            "High-confidence soft values should decode perfectly"
        );
    }

    #[test]
    fn test_coded_length_consistent_all_rates() {
        let rates = [
            CodeRate::Rate1_4,
            CodeRate::Rate1_3,
            CodeRate::Rate1_2,
            CodeRate::Rate2_3,
            CodeRate::Rate3_4,
            CodeRate::Rate7_8,
        ];
        for rate in &rates {
            let mut codec = LdpcCodec::new(*rate);
            let info_bits = codec.code().info_bits();
            let input = vec![0u8; info_bits];
            let encoded = FecCodec::encode(&mut codec, &input);
            assert_eq!(
                encoded.len(),
                1944,
                "Rate {:?}: all codes should produce 1944 coded bits",
                rate
            );
        }
    }

    #[test]
    fn test_info_bits_matches_rate() {
        // Higher rates should have more info bits
        let codec_14 = LdpcCodec::new(CodeRate::Rate1_4);
        let codec_78 = LdpcCodec::new(CodeRate::Rate7_8);
        assert!(
            codec_78.code().info_bits() > codec_14.code().info_bits(),
            "Rate 7/8 should have more info bits than rate 1/4"
        );
    }

    #[test]
    fn test_ldpc_near_threshold_rate_1_4() {
        // Rate 1/4 has the most redundancy — should decode at moderate noise
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let mut codec = LdpcCodec::new(CodeRate::Rate1_4);
        let info_bits = codec.code().info_bits();
        let input: Vec<u8> = (0..info_bits).map(|i| (i % 2) as u8).collect();
        let encoded = FecCodec::encode(&mut codec, &input);

        // Add AWGN at ~2 dB SNR (soft LLRs with noise)
        let snr_linear = 1.585f32; // 10^(2/10)
        let noise_std = (1.0 / (2.0 * snr_linear)).sqrt();
        let mut rng = StdRng::seed_from_u64(77);

        let soft: Vec<f32> = encoded
            .iter()
            .map(|&b| {
                let base = if b == 0 { 1.0 } else { -1.0 };
                let u1: f32 = rng.random::<f32>().max(1e-10);
                let u2: f32 = rng.random();
                let noise =
                    noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                base + noise
            })
            .collect();

        let decoded = FecCodec::decode(&codec, &soft);
        assert_eq!(
            decoded, input,
            "Rate 1/4 should decode at 2 dB SNR (near threshold)"
        );
    }

    #[test]
    fn test_ldpc_beyond_capacity_does_not_panic() {
        // Very high noise — decoder should terminate gracefully, not hang or panic
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let mut codec = LdpcCodec::new(CodeRate::Rate1_2);
        let info_bits = codec.code().info_bits();
        let input: Vec<u8> = (0..info_bits).map(|i| (i % 2) as u8).collect();
        let encoded = FecCodec::encode(&mut codec, &input);

        // Massive noise: -3 dB SNR (decoder will fail to converge)
        let noise_std = 1.5f32;
        let mut rng = StdRng::seed_from_u64(55);

        let soft: Vec<f32> = encoded
            .iter()
            .map(|&b| {
                let base = if b == 0 { 1.0 } else { -1.0 };
                let u1: f32 = rng.random::<f32>().max(1e-10);
                let u2: f32 = rng.random();
                let noise =
                    noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                base + noise
            })
            .collect();

        // Should not panic — output may differ from input, that's expected
        let decoded = FecCodec::decode(&codec, &soft);
        assert_eq!(
            decoded.len(),
            info_bits,
            "Decoder output should have correct length even when it fails to converge"
        );
    }

    /// Generate a soft codeword by adding seeded Box-Muller AWGN of the given
    /// standard deviation to a BPSK-mapped codeword. Mirrors the noise helper
    /// used throughout the existing FEC tests.
    fn awgn_soft(encoded: &[u8], noise_std: f32, seed: u64) -> Vec<f32> {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let mut rng = StdRng::seed_from_u64(seed);
        encoded
            .iter()
            .map(|&b| {
                let base = if b == 0 { 1.0 } else { -1.0 };
                let u1: f32 = rng.random::<f32>().max(1e-10);
                let u2: f32 = rng.random();
                let noise =
                    noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                base + noise
            })
            .collect()
    }

    #[test]
    fn test_ldpc_near_threshold() {
        // Rate 1/2, seeded AWGN near the operating threshold: decode succeeds.
        let mut codec = LdpcCodec::new(CodeRate::Rate1_2);
        let info_bits = codec.code().info_bits();
        let input: Vec<u8> = (0..info_bits).map(|i| ((i * 5 + 1) % 2) as u8).collect();
        let encoded = FecCodec::encode(&mut codec, &input);

        // Moderately strong noise — close to where rate-1/2 starts to fail.
        let soft = awgn_soft(&encoded, 0.6, 1234);
        let (decoded, converged) = codec.decode_checked(&soft);
        assert!(converged, "decoder should converge near threshold");
        assert_eq!(decoded, input, "rate 1/2 should decode near threshold");
    }

    #[test]
    fn test_ldpc_beyond_capacity() {
        // Extremely noisy channel beyond code capacity: the decoder must return
        // a correctly sized result without panicking. Exact match is not required.
        let mut codec = LdpcCodec::new(CodeRate::Rate1_2);
        let info_bits = codec.code().info_bits();
        let input: Vec<u8> = (0..info_bits).map(|i| (i % 2) as u8).collect();
        let encoded = FecCodec::encode(&mut codec, &input);

        let soft = awgn_soft(&encoded, 2.0, 4321);
        let (decoded, _converged) = codec.decode_checked(&soft);
        assert_eq!(
            decoded.len(),
            info_bits,
            "decoder must return correctly sized output even beyond capacity"
        );
    }

    #[test]
    fn test_ldpc_max_iterations_reached() {
        // With heavy noise the decoder will exhaust its iteration budget rather
        // than converge. It must terminate (not hang) and report non-convergence.
        let mut codec = LdpcCodec::new(CodeRate::Rate7_8);
        let info_bits = codec.code().info_bits();
        let input: Vec<u8> = (0..info_bits).map(|i| ((i * 3 + 2) % 2) as u8).collect();
        let encoded = FecCodec::encode(&mut codec, &input);

        // Rate 7/8 has minimal redundancy; this much noise cannot be corrected.
        let soft = awgn_soft(&encoded, 1.5, 2468);
        let (decoded, converged) = codec.decode_checked(&soft);
        assert!(
            !converged,
            "decoder should hit max iterations without converging under heavy noise"
        );
        assert_eq!(
            decoded.len(),
            info_bits,
            "decoder must terminate with correctly sized output at max iterations"
        );
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Deterministic pseudo-random info bits derived from a seed, sized to the
    /// code's info length. Keeps cases reproducible and fast.
    fn info_bits_from_seed(rate: CodeRate, seed: u64) -> (LdpcCodec, Vec<u8>) {
        let codec = LdpcCodec::new(rate);
        let k = codec.code().info_bits();
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let input: Vec<u8> = (0..k)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 63) & 1) as u8
            })
            .collect();
        (codec, input)
    }

    fn rate_strategy() -> impl Strategy<Value = CodeRate> {
        prop_oneof![
            Just(CodeRate::Rate1_4),
            Just(CodeRate::Rate1_3),
            Just(CodeRate::Rate1_2),
            Just(CodeRate::Rate2_3),
            Just(CodeRate::Rate3_4),
            Just(CodeRate::Rate7_8),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Random payload -> encode -> noiseless decode recovers the input,
        /// across all six code rates.
        #[test]
        fn prop_ldpc_roundtrip_all_rates(rate in rate_strategy(), seed in any::<u64>()) {
            let (mut codec, input) = info_bits_from_seed(rate, seed);
            let encoded = FecCodec::encode(&mut codec, &input);
            prop_assert_eq!(encoded.len(), 1944);

            let soft: Vec<f32> = encoded
                .iter()
                .map(|&b| if b == 0 { 1.0 } else { -1.0 })
                .collect();
            let decoded = FecCodec::decode(&codec, &soft);
            prop_assert_eq!(decoded, input);
        }

        /// A freshly encoded codeword is a valid codeword: feeding its
        /// high-confidence LLRs to the decoder converges immediately (all
        /// parity checks satisfied, i.e. H * c = 0).
        #[test]
        fn prop_ldpc_codeword_validity(rate in rate_strategy(), seed in any::<u64>()) {
            let (mut codec, input) = info_bits_from_seed(rate, seed);
            let encoded = FecCodec::encode(&mut codec, &input);

            let soft: Vec<f32> = encoded
                .iter()
                .map(|&b| if b == 0 { 4.0 } else { -4.0 })
                .collect();
            let (_decoded, converged) = codec.decode_checked(&soft);
            prop_assert!(converged, "encoded codeword must satisfy all parity checks");
        }
    }
}
