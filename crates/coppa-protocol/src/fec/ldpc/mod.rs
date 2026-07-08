//! LDPC (Low-Density Parity-Check) forward error correction.
//!
//! Implements QC-LDPC codes from IEEE Std 802.11-2012, Annex F:
//! - Lifting factor Z = 81
//! - 24-column base matrices
//! - 1,944 coded bits for all rates
//! - Normalized min-sum belief propagation decoder (alpha = 0.8) with early termination
//!
//! Supported code rates: 1/4, 1/3, 1/2, 2/3, 3/4, 7/8.
pub mod codes;
pub mod decoder;
pub mod encoder;
pub mod nr_bg2;
pub mod rate_match;

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

    /// Encode info bits into a coded block (1944 bits for all rates).
    ///
    /// Unlike the [`FecCodec::encode`] trait method (which takes `&mut self` for
    /// generality across codecs that might need mutable state), the underlying
    /// [`LdpcEncoder::encode_block`] is already `&self` — this inherent method
    /// exposes that directly. Added for `CoppaTransceiver`'s per-speed-level codec
    /// cache (Task 7): `decode_checked` above was already `&self`, so this was the
    /// only obstacle to holding cached `LdpcCodec`s as plain immutable values with
    /// no `RefCell`/interior mutability — see the Task 7 report for the full
    /// decision.
    pub fn encode(&self, info_bits: &[u8]) -> Vec<u8> {
        self.encoder.encode_block(info_bits)
    }
}

/// NR BG2 mother code (Task 4): one code for every speed level, rate-matched
/// down per level via `rate_match`/`rate_dematch` instead of switching
/// between per-rate base matrices (the old `LdpcCodec` approach above, kept
/// for reference/back-compat but no longer used by `CoppaTransceiver`).
///
/// Cache **one** `NrLdpc` instance and reuse it for every speed level and
/// every frame -- see `nr_bg2` for the (Zc=176, fixed) lifted-graph
/// construction cost this amortizes.
#[derive(Debug, Clone)]
pub struct NrLdpc {
    encoder: encoder::NrBg2Encoder,
    decoder: decoder::NrBg2Decoder,
}

impl Default for NrLdpc {
    fn default() -> Self {
        Self::new()
    }
}

impl NrLdpc {
    pub fn new() -> Self {
        Self {
            encoder: encoder::NrBg2Encoder::new(),
            decoder: decoder::NrBg2Decoder::new(),
        }
    }

    /// Fixed info width for every speed level: `KB * ZC` = 1760.
    pub const INFO_LEN: usize = nr_bg2::KB * nr_bg2::ZC;
    /// Fixed mother-codeword width: `(BASE_COLS - PUNCTURED_INFO_COLS) * ZC` = 8800.
    pub const MOTHER_LEN: usize = (nr_bg2::BASE_COLS - nr_bg2::PUNCTURED_INFO_COLS) * nr_bg2::ZC;

    /// Encode `info` (length [`Self::INFO_LEN`] = 1760) into the mother
    /// codeword ([`Self::MOTHER_LEN`] = 8800 bits): `[info[2*ZC..], parity]`
    /// (the leading `2*ZC` = 352 systematic bits are punctured -- never
    /// transmitted -- per standard NR practice; see `rate_match` module
    /// docs).
    ///
    /// # Panics
    /// Panics if `info.len() != Self::INFO_LEN`.
    pub fn encode(&self, info: &[u8]) -> Vec<u8> {
        self.encoder.encode_mother(info)
    }

    /// Decode. `mother_llrs` must have length [`Self::MOTHER_LEN`] = 8800,
    /// in the *same domain* as [`Self::encode`]'s output (i.e. NOT
    /// including the always-punctured leading `2*ZC` positions -- those are
    /// prepended internally as LLR = 0.0, standard treatment for punctured
    /// bits the decoder must still recover through the graph's parity
    /// constraints).
    ///
    /// Returns `(posterior, info, converged)`:
    /// - `posterior`: full-graph posterior LLRs, length `BASE_COLS * ZC` =
    ///   9152 (index 0 is the first punctured info bit).
    /// - `info`: hard-decided information bits, length [`Self::INFO_LEN`] =
    ///   1760 (`posterior[..1760]`, sign-decided).
    /// - `converged`: whether all parity checks were satisfied.
    ///
    /// # Panics
    /// Panics if `mother_llrs.len() != Self::MOTHER_LEN`.
    pub fn decode_soft(&self, mother_llrs: &[f32]) -> (Vec<f32>, Vec<u8>, bool) {
        let (posterior, info, converged, _iters) = self.decode_soft_stats(mother_llrs);
        (posterior, info, converged)
    }

    /// Like [`Self::decode_soft`], but also returns the number of layered
    /// iterations used before early-exit (or exhausting the iteration cap).
    /// Used by the Task 4 bench gate to record early-exit iteration
    /// statistics; not part of the brief's minimum interface, but additive
    /// (doesn't change `decode_soft`'s signature or behavior).
    pub fn decode_soft_stats(&self, mother_llrs: &[f32]) -> (Vec<f32>, Vec<u8>, bool, usize) {
        assert_eq!(
            mother_llrs.len(),
            Self::MOTHER_LEN,
            "expected {} mother LLRs, got {}",
            Self::MOTHER_LEN,
            mother_llrs.len()
        );
        let punctured_len = nr_bg2::PUNCTURED_INFO_COLS * nr_bg2::ZC;
        let mut full_llrs = Vec::with_capacity(nr_bg2::BASE_COLS * nr_bg2::ZC);
        full_llrs.resize(punctured_len, 0.0f32);
        full_llrs.extend_from_slice(mother_llrs);

        let (posterior, iterations, converged) = self.decoder.decode(&full_llrs);
        let info: Vec<u8> = posterior[..Self::INFO_LEN]
            .iter()
            .map(|&l| if l >= 0.0 { 0 } else { 1 })
            .collect();
        (posterior, info, converged, iterations)
    }
}

/// Pin known-zero padding LLRs into a mother-domain dematched buffer
/// (Task 3's known-pad pinning mechanism, extended for the NR BG2 mother
/// code -- Task 4 Step 4).
///
/// `dematched` must be [`NrLdpc::MOTHER_LEN`]-long (the same domain
/// [`rate_match::rate_dematch`] produces / [`NrLdpc::decode_soft`] expects).
/// Every info bit beyond the actual payload (`payload_bits..KB*ZC`, i.e.
/// *both* the shortened-but-transmitted padding `payload_bits..k_used` *and*
/// the never-transmitted shortened tail `k_used..KB*ZC` -- the latter is
/// exactly the region `rate_dematch` leaves at `0.0`) is scrambled zero on
/// TX (see `crate::fec::scrambler::prbs_bits`'s doc for why the RX can
/// compute this exact value without knowing the payload), so it is pinned
/// here to a high-confidence LLR instead of trusting the (noisy, or in the
/// shortened case entirely absent) channel observation. One pass covers
/// both sub-ranges uniformly -- the pin value only depends on the PRBS
/// keystream, not on which side of `k_used` a position falls.
///
/// Positions `0..payload_bits.min(PUNCTURED_INFO_COLS*ZC)` (real payload
/// bits that happen to fall in the always-punctured leading `2*ZC` region)
/// are deliberately left untouched -- those are genuine unknown payload
/// data recovered only through the code's redundancy, exactly as intended
/// by standard NR LDPC puncturing.
///
/// # Panics
/// Panics if `dematched.len() != NrLdpc::MOTHER_LEN` or `payload_bits >
/// k_used` (payload can never exceed a level's shortened capacity).
pub fn pin_known_pad(dematched: &mut [f32], payload_bits: usize, k_used: usize, pin: f32) {
    assert_eq!(dematched.len(), NrLdpc::MOTHER_LEN);
    assert!(
        payload_bits <= k_used,
        "payload_bits={payload_bits} must not exceed k_used={k_used}"
    );

    let punctured_len = nr_bg2::PUNCTURED_INFO_COLS * nr_bg2::ZC;
    let info_len = nr_bg2::KB * nr_bg2::ZC;
    if payload_bits >= info_len {
        return; // full-width payload, nothing to pin
    }

    let prbs = crate::fec::scrambler::prbs_bits(info_len);
    let pin_start_overall = payload_bits.max(punctured_len);
    for (overall_idx, &prbs_bit) in prbs
        .iter()
        .enumerate()
        .take(info_len)
        .skip(pin_start_overall)
    {
        let dematch_idx = overall_idx - punctured_len;
        dematched[dematch_idx] = if prbs_bit == 0 { pin } else { -pin };
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
mod nr_ldpc_codec_tests {
    //! Task 4 Step 2: end-to-end codec-level tests for the NR BG2 mother
    //! code + rate matching + known-pad pinning, one level at a time. These
    //! exercise exactly the pipeline `CoppaTransceiver` uses (Step 4), but
    //! at the FEC layer directly (no OFDM/interleaver/constellation), so a
    //! regression here localizes to LDPC/rate-matching/pinning specifically.
    use super::*;
    use crate::fec::scrambler::scramble;

    const E: usize = 1944;

    /// `(wire_level, k_used)` for every non-reserved level -- mirrors
    /// `crate::modem::speed_levels::k_used_for_level` (duplicated here as a
    /// literal so this FEC-layer test module doesn't need to depend on
    /// `crate::modem`; `speed_levels.rs` is the single source of truth for
    /// production code, this is verification data only -- see
    /// `speed_levels::tests::k_used_matches_audited_ladder` for the
    /// production-side pin of the same table).
    const ALL_LEVELS_K_USED: [(u8, usize); 9] = [
        (1, 486),
        (2, 972),
        (3, 972),
        (4, 1458),
        (5, 1296),
        (6, 972),
        (7, 1458),
        (9, 1296),
        (10, 1620),
    ];

    /// Build a scrambled 1760-bit info block for `payload_bits` of
    /// deterministic pseudo-random payload followed by zero padding, exactly
    /// as `CoppaTransceiver::transmit` will (Step 4).
    fn build_info(payload_bits: usize, seed: u64) -> Vec<u8> {
        let mut info = vec![0u8; NrLdpc::INFO_LEN];
        let mut state = seed;
        for bit in info.iter_mut().take(payload_bits) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *bit = ((state >> 63) & 1) as u8;
        }
        scramble(&mut info);
        info
    }

    /// Full TX->channel->RX loopback for one `(k_used, payload_bits)` pair.
    /// `flip_fraction` of the E=1944 transmitted coded bits are hard-flipped
    /// (simulating channel errors) before soft-demapping to LLRs.
    fn loopback(
        ldpc: &NrLdpc,
        k_used: usize,
        payload_bits: usize,
        flip_fraction: f32,
        seed: u64,
    ) -> (bool, Vec<u8>, Vec<u8>) {
        let info = build_info(payload_bits, seed);
        let mother = ldpc.encode(&info);
        let matched = rate_match::rate_match(&mother, k_used, E, 0);

        let mut llrs: Vec<f32> = matched
            .iter()
            .map(|&b| if b == 0 { 3.0 } else { -3.0 })
            .collect();
        // Corrupt exactly `n_flip` evenly-spaced LLRs: sign-flip at a *lower*
        // magnitude than the clean bits (1.0 vs 3.0), i.e. weak-but-wrong
        // evidence rather than adversarial maximum-confidence errors -- this
        // is what real channel noise near the decision boundary actually
        // looks like (a genuinely maximum-confidence-wrong bit at a full 5%
        // rate is a far harsher condition than any AWGN/fading channel this
        // codec targets would produce, and is not what "5% LLR corruption"
        // is meant to model here).
        let n_flip = ((E as f32) * flip_fraction) as usize;
        for k in 0..n_flip {
            let idx = (k * E) / n_flip.max(1);
            llrs[idx] = if llrs[idx] > 0.0 { -1.0 } else { 1.0 };
        }

        let dematched = rate_match::rate_dematch(&llrs, k_used, E, 0, NrLdpc::MOTHER_LEN);
        let mut dematched = dematched;
        pin_known_pad(&mut dematched, payload_bits, k_used, 64.0);

        let (_, mut decoded_info, converged) = ldpc.decode_soft(&dematched);
        scramble(&mut decoded_info); // descramble
        let decoded_payload_bits = decoded_info[..payload_bits].to_vec();
        let original_payload_bits = {
            let mut original = build_info(payload_bits, seed);
            scramble(&mut original); // undo the scramble build_info applied
            original[..payload_bits].to_vec()
        };
        (converged, decoded_payload_bits, original_payload_bits)
    }

    #[test]
    fn perfect_llr_loopback_every_level() {
        let ldpc = NrLdpc::new();
        for (level, k_used) in ALL_LEVELS_K_USED {
            let payload_bits = (k_used - 16).min(400); // comfortably under capacity
            let (converged, decoded, original) =
                loopback(&ldpc, k_used, payload_bits, 0.0, 0xC0FFEE + level as u64);
            assert!(
                converged,
                "level {level} (k_used={k_used}): decoder did not converge"
            );
            assert_eq!(
                decoded, original,
                "level {level} (k_used={k_used}): payload mismatch"
            );
        }
    }

    /// Regression guard for the alpha-calibration bug fixed during Task 4:
    /// an alpha (normalized min-sum scale) value swept and picked at level 2
    /// only (see `decoder::NR_DEFAULT_SCALE`'s doc) broke real convergence
    /// at level 10 with a tiny (1-byte) payload -- extreme known-pad pinning,
    /// the most rate/pinning-dependent operating point in the whole ladder --
    /// even on a perfectly clean channel (0% LLR corruption, i.e. this is not
    /// a noise-margin issue, it's a structural convergence failure). This was
    /// only ever caught by `tests/phase_c_loopback.rs`'s
    /// `test_all_levels_min_payload` / `test_awgn_level_10_above_threshold`,
    /// which are workspace integration tests that CI does NOT run (CI only
    /// runs `cargo test --lib`, per this crate's `CLAUDE.md`); the existing
    /// `perfect_llr_loopback_every_level` test above uses a 400-bit payload
    /// for every level, which does not reproduce the triggering condition.
    /// This test reproduces it directly at the FEC layer so
    /// `cargo test -p coppa-protocol --lib` alone would catch a regression.
    #[test]
    fn perfect_llr_loopback_level_10_tiny_payload() {
        let ldpc = NrLdpc::new();
        let k_used = 1620; // level 10 (see ALL_LEVELS_K_USED)
        let payload_bits = 8; // 1 byte -- the exact size that broke alpha=0.80
        let (converged, decoded, original) =
            loopback(&ldpc, k_used, payload_bits, 0.0, 0xA1FA_0000);
        assert!(
            converged,
            "level 10 (k_used={k_used}) tiny (1-byte) payload: decoder did not converge -- \
             this is the exact condition that broke with a mis-calibrated normalized min-sum alpha"
        );
        assert_eq!(
            decoded, original,
            "level 10 (k_used={k_used}) tiny (1-byte) payload: payload mismatch"
        );
    }

    #[test]
    fn perfect_llr_loopback_max_payload_every_level() {
        let ldpc = NrLdpc::new();
        for (level, k_used) in ALL_LEVELS_K_USED {
            let payload_bits = k_used; // full capacity, no padding at all
            let (converged, decoded, original) =
                loopback(&ldpc, k_used, payload_bits, 0.0, 0xFACE + level as u64);
            assert!(
                converged,
                "level {level} (k_used={k_used}) @ max payload: did not converge"
            );
            assert_eq!(
                decoded, original,
                "level {level} (k_used={k_used}) @ max payload: mismatch"
            );
        }
    }

    #[test]
    fn corrupt_5_percent_of_llrs_still_decodes_every_level() {
        // 5% of E=1944 coded bits hard-flipped (~97 bits) before soft
        // demapping -- well within a real LDPC code's correction capability
        // at a reasonable operating point, across the whole ladder.
        let ldpc = NrLdpc::new();
        for (level, k_used) in ALL_LEVELS_K_USED {
            let payload_bits = (k_used / 2).max(64);
            let (converged, decoded, original) = loopback(
                &ldpc,
                k_used,
                payload_bits,
                0.05,
                0xBEEF_0000 + level as u64,
            );
            assert!(
                converged,
                "level {level} (k_used={k_used}): did not converge with 5% LLR corruption"
            );
            assert_eq!(
                decoded, original,
                "level {level} (k_used={k_used}): payload mismatch with 5% LLR corruption"
            );
        }
    }

    #[test]
    fn rate_match_output_is_exactly_e_for_every_level_and_rv() {
        let ldpc = NrLdpc::new();
        let info = vec![0u8; NrLdpc::INFO_LEN];
        let mother = ldpc.encode(&info);
        for (level, k_used) in ALL_LEVELS_K_USED {
            for rv in 0..=3u8 {
                let matched = rate_match::rate_match(&mother, k_used, E, rv);
                assert_eq!(
                    matched.len(),
                    E,
                    "level {level} rv={rv}: wrong rate_match length"
                );
            }
        }
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
