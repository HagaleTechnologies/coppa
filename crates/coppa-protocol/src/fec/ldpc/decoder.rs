//! Offset min-sum belief propagation decoder for QC-LDPC codes.
//!
//! The offset min-sum algorithm is a reduced-complexity approximation to the
//! standard sum-product (belief propagation) decoder. Instead of computing
//! exact hyperbolic tangent functions for check-node updates, it uses:
//!
//!   min(|L_1|, |L_2|, ...) - beta
//!
//! where beta is a small positive offset (typically 0.5) that compensates for
//! the approximation error. This provides nearly identical error-correction
//! performance to sum-product while using only additions, comparisons, and
//! sign operations -- no transcendental functions.
//!
//! The decoder operates on log-likelihood ratios (LLRs):
//!   - Positive LLR => bit is more likely 0
//!   - Negative LLR => bit is more likely 1
//!
//! Early termination: after each iteration, the decoder checks if the current
//! hard decision satisfies all parity checks. If so, it stops immediately.
use super::codes::{LdpcCode, LIFTING_FACTOR};

/// Default scaling factor for the normalized min-sum approximation.
///
/// Min-sum overestimates check-node reliability; a multiplicative factor in (0,1] corrects
/// it. Crucially, normalized min-sum is **scale-invariant** (scaling all input LLRs by `c`
/// scales every message by `c`, leaving the hard decisions unchanged) — unlike the previous
/// fixed *offset* min-sum, which annihilated the small LLRs that fading produces and silently
/// discarded correctable frames. See `decoder_is_scale_invariant`.
const DEFAULT_SCALE: f32 = 0.8;

/// Default maximum number of decoding iterations.
const DEFAULT_MAX_ITERATIONS: usize = 50;

/// Precomputed edge structure for efficient message passing.
///
/// Organizes edges by check nodes and variable nodes for fast iteration
/// during the BP update steps.
#[derive(Debug, Clone)]
struct TannerGraph {
    /// For each check node: list of (edge_index, variable_node) pairs.
    check_to_edges: Vec<Vec<(usize, usize)>>,
    /// For each variable node: list of (edge_index, check_node) pairs.
    var_to_edges: Vec<Vec<(usize, usize)>>,
    /// Total number of edges (non-zero entries in H).
    num_edges: usize,
    /// Number of check nodes (rows in H).
    num_checks: usize,
    /// Number of variable nodes (columns in H = coded bits).
    #[allow(dead_code)]
    num_vars: usize,
}

impl TannerGraph {
    /// Build the Tanner graph from the LDPC code's parity check matrix.
    fn from_code(code: &LdpcCode) -> Self {
        let z = LIFTING_FACTOR;
        let num_checks = code.base_rows() * z;
        let num_vars = code.base_cols() * z;

        // Count total edges
        let num_edges = code.entries().len() * z;

        let mut check_to_edges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); num_checks];
        let mut var_to_edges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); num_vars];

        let mut edge_idx = 0;
        for entry in code.entries() {
            for i in 0..z {
                let check = entry.base_row * z + i;
                let var = entry.base_col * z + (i + entry.shift) % z;

                check_to_edges[check].push((edge_idx, var));
                var_to_edges[var].push((edge_idx, check));
                edge_idx += 1;
            }
        }

        debug_assert_eq!(edge_idx, num_edges);

        Self {
            check_to_edges,
            var_to_edges,
            num_edges,
            num_checks,
            num_vars,
        }
    }
}

/// Offset min-sum belief propagation LDPC decoder.
#[derive(Debug, Clone)]
pub struct LdpcDecoder {
    code: LdpcCode,
    graph: TannerGraph,
    /// Scaling factor for the normalized min-sum approximation (in (0,1]).
    scale: f32,
    /// Maximum number of BP iterations before giving up.
    max_iterations: usize,
}

impl LdpcDecoder {
    /// Create a new decoder with default parameters (normalized min-sum scale=0.8, max_iter=50).
    pub fn new(code: LdpcCode) -> Self {
        let graph = TannerGraph::from_code(&code);
        Self {
            code,
            graph,
            scale: DEFAULT_SCALE,
            max_iterations: DEFAULT_MAX_ITERATIONS,
        }
    }

    /// Create a decoder with a custom normalized-min-sum `scale` (in (0,1]) and iteration cap.
    pub fn with_params(code: LdpcCode, scale: f32, max_iterations: usize) -> Self {
        let graph = TannerGraph::from_code(&code);
        Self {
            code,
            graph,
            scale,
            max_iterations,
        }
    }

    /// Returns a reference to the underlying code.
    pub fn code(&self) -> &LdpcCode {
        &self.code
    }

    /// Decode a block of soft LLR values to information bits.
    ///
    /// Input: `llrs` of length `code.coded_bits()`.
    ///   - Positive LLR => bit more likely 0
    ///   - Negative LLR => bit more likely 1
    ///
    /// Output: decoded information bits of length `code.info_bits()`.
    ///
    /// The decoder runs offset min-sum BP for up to `max_iterations`,
    /// terminating early if all parity checks are satisfied.
    ///
    /// Note: if you need to know whether the decoder converged (all parity
    /// checks satisfied), use `decode_block_checked` instead.
    pub fn decode_block(&self, llrs: &[f32]) -> Vec<u8> {
        self.decode_block_checked(llrs).0
    }

    /// Decode a block, also returning whether the decoder converged.
    ///
    /// Returns `(decoded_info_bits, converged)` where `converged` is `true`
    /// if all parity checks were satisfied before hitting `max_iterations`.
    /// When `converged` is `false`, the output is a best-effort decode and
    /// may contain residual errors.
    pub fn decode_block_checked(&self, llrs: &[f32]) -> (Vec<u8>, bool) {
        let n = self.code.coded_bits();
        let k = self.code.info_bits();

        assert_eq!(llrs.len(), n, "Expected {} LLRs, got {}", n, llrs.len());

        self.decode_block_inner(llrs, n, k)
    }

    /// Decode a block, returning an error on length mismatch instead of panicking.
    pub fn try_decode_block(&self, llrs: &[f32]) -> Result<Vec<u8>, String> {
        let n = self.code.coded_bits();
        let k = self.code.info_bits();

        if llrs.len() != n {
            return Err(format!("Expected {} LLRs, got {}", n, llrs.len()));
        }

        Ok(self.decode_block_inner(llrs, n, k).0)
    }

    fn decode_block_inner(&self, llrs: &[f32], n: usize, k: usize) -> (Vec<u8>, bool) {
        let num_edges = self.graph.num_edges;

        // Messages from check nodes to variable nodes (indexed by edge).
        let mut check_to_var: Vec<f32> = vec![0.0; num_edges];
        // Messages from variable nodes to check nodes (indexed by edge).
        let mut var_to_check: Vec<f32> = vec![0.0; num_edges];

        // Initialize variable-to-check messages with channel LLRs
        for (var, edges) in self.graph.var_to_edges.iter().enumerate() {
            for &(edge_idx, _check) in edges {
                var_to_check[edge_idx] = llrs[var];
            }
        }

        // Total belief (posterior LLR) for each variable node
        let mut total_llr = vec![0.0f32; n];
        let mut converged = false;

        for _iter in 0..self.max_iterations {
            // === Check node update (horizontal step) ===
            // For each check node, compute outgoing messages using offset min-sum.
            for check in 0..self.graph.num_checks {
                let edges = &self.graph.check_to_edges[check];
                let num_neighbors = edges.len();
                if num_neighbors == 0 {
                    continue;
                }

                // Compute product of signs and minimum magnitudes
                // For each outgoing edge e, the message is:
                //   sign = product of signs of all OTHER incoming messages
                //   magnitude = max(min of all OTHER magnitudes - offset, 0)

                // Precompute signs and magnitudes (reuse stack-local buffers)
                // Max check node degree is bounded by the base matrix structure
                let mut signs = [0.0f32; 32];
                let mut magnitudes = [0.0f32; 32];
                // B6: Use assert! (not debug_assert!) so this bounds check runs in
                // release builds, preventing undefined out-of-bounds writes.
                assert!(
                    num_neighbors <= 32,
                    "check node degree {} exceeds fixed buffer size 32",
                    num_neighbors
                );

                for (j, &(edge_idx, _var)) in edges.iter().enumerate() {
                    let msg = var_to_check[edge_idx];
                    signs[j] = if msg >= 0.0 { 1.0 } else { -1.0 };
                    magnitudes[j] = msg.abs();
                }

                // Product of all signs
                let total_sign: f32 = signs[..num_neighbors].iter().product();

                // Find the two smallest magnitudes for efficient exclusion
                let (min1_val, min1_idx, min2_val) = two_smallest(&magnitudes[..num_neighbors]);

                for (local_idx, &(edge_idx, _var)) in edges.iter().enumerate() {
                    // Sign: total product divided by this edge's sign
                    let outgoing_sign = total_sign * signs[local_idx];

                    // Magnitude: minimum of all OTHER magnitudes, scaled (normalized min-sum).
                    let min_other = if local_idx == min1_idx {
                        min2_val
                    } else {
                        min1_val
                    };
                    let mag = min_other * self.scale;

                    check_to_var[edge_idx] = outgoing_sign * mag;
                }
            }

            // === Variable node update (vertical step) ===
            // For each variable node, compute total LLR and outgoing messages.
            for (var, edges) in self.graph.var_to_edges.iter().enumerate() {
                let channel = llrs[var];

                // Total LLR = channel LLR + sum of all incoming check-to-var messages
                let incoming_sum: f32 = edges
                    .iter()
                    .map(|&(edge_idx, _check)| check_to_var[edge_idx])
                    .sum();
                total_llr[var] = channel + incoming_sum;

                // Outgoing messages: total LLR minus the incoming message from target check
                for &(edge_idx, _check) in edges {
                    var_to_check[edge_idx] = total_llr[var] - check_to_var[edge_idx];
                }
            }

            // === Early termination check ===
            // Make hard decisions and check if all parity checks are satisfied.
            if self.check_syndrome(&total_llr) {
                converged = true;
                break;
            }
        }

        // Extract information bits from hard decisions on total LLR
        let decoded: Vec<u8> = total_llr[..k]
            .iter()
            .map(|&l| if l >= 0.0 { 0 } else { 1 })
            .collect();
        (decoded, converged)
    }

    /// Check if the hard decision from current LLRs satisfies all parity checks.
    fn check_syndrome(&self, total_llr: &[f32]) -> bool {
        for check in 0..self.graph.num_checks {
            let mut syndrome = 0u8;
            for &(_edge_idx, var) in &self.graph.check_to_edges[check] {
                let hard_bit = if total_llr[var] >= 0.0 { 0u8 } else { 1u8 };
                syndrome ^= hard_bit;
            }
            if syndrome != 0 {
                return false;
            }
        }
        true
    }
}

/// Find the two smallest values in a slice, returning (min1, min1_index, min2).
/// Used for efficient check node updates where we need the minimum excluding
/// one element.
fn two_smallest(values: &[f32]) -> (f32, usize, f32) {
    let mut min1 = f32::MAX;
    let mut min1_idx = 0;
    let mut min2 = f32::MAX;

    for (i, &v) in values.iter().enumerate() {
        if v < min1 {
            min2 = min1;
            min1 = v;
            min1_idx = i;
        } else if v < min2 {
            min2 = v;
        }
    }

    (min1, min1_idx, min2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fec::ldpc::codes::CodeRate;
    use crate::fec::ldpc::encoder::LdpcEncoder;

    fn encode_and_make_soft(encoder: &LdpcEncoder, info: &[u8]) -> Vec<f32> {
        let cw = encoder.encode_block(info);
        cw.iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect()
    }

    #[test]
    fn test_decode_perfect_channel() {
        for rate in &[
            CodeRate::Rate1_4,
            CodeRate::Rate1_3,
            CodeRate::Rate1_2,
            CodeRate::Rate2_3,
            CodeRate::Rate3_4,
            CodeRate::Rate7_8,
        ] {
            let code = LdpcCode::new(*rate);
            let enc = LdpcEncoder::new(code.clone());
            let dec = LdpcDecoder::new(code.clone());

            let info: Vec<u8> = (0..code.info_bits()).map(|i| (i % 2) as u8).collect();
            let soft = encode_and_make_soft(&enc, &info);
            let decoded = dec.decode_block(&soft);

            assert_eq!(decoded, info, "{:?}: perfect channel decode failed", rate);
        }
    }

    #[test]
    fn test_decode_with_noise() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code.clone());
        let dec = LdpcDecoder::new(code.clone());

        let info: Vec<u8> = (0..code.info_bits())
            .map(|i| ((i * 7 + 13) % 2) as u8)
            .collect();
        let cw = enc.encode_block(&info);

        // Add seeded Gaussian noise (~4 dB Eb/N0)
        let noise_std = 0.5f32;
        let mut rng = StdRng::seed_from_u64(42);

        let soft: Vec<f32> = cw
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

        let decoded = dec.decode_block(&soft);
        assert_eq!(decoded, info, "Rate 1/2 should correct moderate noise");
    }

    #[test]
    fn test_decode_with_bit_flips() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code.clone());
        let dec = LdpcDecoder::new(code.clone());

        let info: Vec<u8> = (0..code.info_bits())
            .map(|i| ((i * 3 + 5) % 2) as u8)
            .collect();
        let cw = enc.encode_block(&info);

        // Flip a small number of bits (simulate hard errors)
        // With rate 1/2 and 972 parity bits, we should handle a few flips.
        let flip_positions: Vec<usize> = (0..10).map(|i| i * 37 % cw.len()).collect();
        let soft: Vec<f32> = cw
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                let bit = if flip_positions.contains(&i) {
                    b ^ 1
                } else {
                    b
                };
                if bit == 0 {
                    2.0
                } else {
                    -2.0
                }
            })
            .collect();

        let decoded = dec.decode_block(&soft);
        assert_eq!(
            decoded,
            info,
            "Rate 1/2 should correct {} hard bit flips",
            flip_positions.len()
        );
    }

    #[test]
    fn test_early_termination() {
        // With perfect channel, the decoder should terminate in very few iterations
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code.clone());
        // Use max_iterations=1 -- with perfect channel, one iteration should suffice
        let dec = LdpcDecoder::with_params(code.clone(), 0.5, 2);

        let info = vec![0u8; code.info_bits()];
        let soft = encode_and_make_soft(&enc, &info);
        let decoded = dec.decode_block(&soft);

        assert_eq!(
            decoded, info,
            "Perfect channel should decode in very few iterations"
        );
    }

    #[test]
    fn test_decoder_output_length() {
        for rate in &[
            CodeRate::Rate1_4,
            CodeRate::Rate1_3,
            CodeRate::Rate1_2,
            CodeRate::Rate2_3,
            CodeRate::Rate3_4,
            CodeRate::Rate7_8,
        ] {
            let code = LdpcCode::new(*rate);
            let dec = LdpcDecoder::new(code.clone());

            let llrs = vec![1.0f32; code.coded_bits()];
            let decoded = dec.decode_block(&llrs);
            assert_eq!(
                decoded.len(),
                code.info_bits(),
                "{:?}: wrong output length",
                rate
            );
        }
    }

    #[test]
    fn test_two_smallest() {
        let vals = vec![3.0, 1.0, 4.0, 1.5, 2.0];
        let (min1, min1_idx, min2) = two_smallest(&vals);
        assert!((min1 - 1.0).abs() < 1e-6);
        assert_eq!(min1_idx, 1);
        assert!((min2 - 1.5).abs() < 1e-6);
    }

    #[test]
    fn test_two_smallest_same_values() {
        let vals = vec![2.0, 2.0, 2.0];
        let (min1, _idx, min2) = two_smallest(&vals);
        assert!((min1 - 2.0).abs() < 1e-6);
        assert!((min2 - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_custom_scale() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code.clone());

        // Normalized min-sum with different scaling factors — all decode a perfect channel.
        for &scale in &[0.5, 0.75, 0.8, 1.0] {
            let dec = LdpcDecoder::with_params(code.clone(), scale, 50);
            let info = vec![0u8; code.info_bits()];
            let soft = encode_and_make_soft(&enc, &info);
            let decoded = dec.decode_block(&soft);
            assert_eq!(
                decoded, info,
                "scale={}: perfect channel should always work",
                scale
            );
        }
    }

    #[test]
    fn decoder_is_scale_invariant() {
        // Regression test for the offset-min-sum scale bug: a faded HF frame yields
        // correct-sign but tiny LLRs, and the fixed 0.5 offset annihilated them, discarding
        // correctable frames. A correct decoder must give the SAME result regardless of the
        // overall LLR magnitude — decoding identical-sign LLRs at unit scale and at 0.01x
        // scale must both converge to the same codeword.
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code.clone());
        let dec = LdpcDecoder::new(code.clone());

        let info: Vec<u8> = (0..code.info_bits()).map(|i| (i % 3 == 0) as u8).collect();
        let coded = enc.encode_block(&info);
        // Correct-sign unit LLRs, then flip a handful of signs (errors within rate-1/2 capacity).
        let mut llrs: Vec<f32> = coded
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        for &i in &[5usize, 99, 333, 700, 1500] {
            llrs[i] = -llrs[i];
        }

        let (d_unit, c_unit) = dec.decode_block_checked(&llrs);
        let small: Vec<f32> = llrs.iter().map(|x| x * 0.01).collect();
        let (d_small, c_small) = dec.decode_block_checked(&small);

        assert!(
            c_unit && d_unit == info,
            "must converge+correct at unit LLR scale"
        );
        assert!(
            c_small && d_small == info,
            "must ALSO converge+correct at 0.01x LLR scale (scale invariance)"
        );
    }

    #[test]
    fn test_syndrome_check() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let dec = LdpcDecoder::new(code.clone());

        // Valid codeword should pass syndrome check
        let enc = LdpcEncoder::new(code.clone());
        let info = vec![0u8; code.info_bits()];
        let soft = encode_and_make_soft(&enc, &info);
        assert!(dec.check_syndrome(&soft));
    }

    #[test]
    fn test_all_ones_info() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code.clone());
        let dec = LdpcDecoder::new(code.clone());

        let info = vec![1u8; code.info_bits()];
        let soft = encode_and_make_soft(&enc, &info);
        let decoded = dec.decode_block(&soft);
        assert_eq!(decoded, info, "All-ones info should roundtrip correctly");
    }

    #[test]
    fn test_low_rate_strong_correction() {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        // Rate 1/4 has the most redundancy -- should correct more errors
        let code = LdpcCode::new(CodeRate::Rate1_4);
        let enc = LdpcEncoder::new(code.clone());
        let dec = LdpcDecoder::new(code.clone());

        let info: Vec<u8> = (0..code.info_bits())
            .map(|i| ((i * 11 + 7) % 2) as u8)
            .collect();
        let cw = enc.encode_block(&info);

        // Heavier noise than rate 1/2 test
        let noise_std = 0.7f32;
        let mut rng = StdRng::seed_from_u64(99);

        let soft: Vec<f32> = cw
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

        let decoded = dec.decode_block(&soft);
        assert_eq!(decoded, info, "Rate 1/4 should handle heavy noise");
    }

    #[test]
    fn test_high_rate_roundtrip() {
        // Rate 7/8 has minimal redundancy but should still work on clean channel
        let code = LdpcCode::new(CodeRate::Rate7_8);
        let enc = LdpcEncoder::new(code.clone());
        let dec = LdpcDecoder::new(code.clone());

        let info: Vec<u8> = (0..code.info_bits())
            .map(|i| ((i * 13 + 1) % 2) as u8)
            .collect();
        let soft = encode_and_make_soft(&enc, &info);
        let decoded = dec.decode_block(&soft);
        assert_eq!(decoded, info, "Rate 7/8 should work on clean channel");
    }

    #[test]
    #[should_panic(expected = "Expected")]
    fn test_wrong_input_length() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let dec = LdpcDecoder::new(code);
        dec.decode_block(&[1.0, -1.0, 0.5]); // Too short
    }
}
