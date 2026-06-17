//! Systematic LDPC encoder.
//!
//! Produces codewords of the form [info_bits | parity_bits] where the parity bits
//! are computed from the parity check matrix H such that H * c = 0 (mod 2).
//!
//! Encoding exploits the staircase (dual-diagonal) structure of the parity portion
//! of the base matrix. For a QC-LDPC code with parity matrix structured as:
//!
//!   H = [A | B]
//!
//! where A covers information columns and B covers parity columns with approximate
//! staircase form, we solve B * p = A * s (mod 2) for parity bits p given info bits s.
//!
//! The staircase structure of B allows efficient back-substitution encoding in
//! O(N * row_weight) time rather than O(N^2) Gaussian elimination.
use super::codes::{LdpcCode, LIFTING_FACTOR};

/// Systematic LDPC encoder.
#[derive(Debug, Clone)]
pub struct LdpcEncoder {
    code: LdpcCode,
}

impl LdpcEncoder {
    /// Create a new encoder for the given LDPC code.
    ///
    /// # Panics
    /// Panics if the code's parity sub-matrix does not have valid staircase ordering,
    /// i.e., if any non-diagonal parity entry in a base row references a later row.
    pub fn new(code: LdpcCode) -> Self {
        Self::try_new(code).expect("Invalid LDPC code: staircase invariant violated")
    }

    /// Create a new encoder, returning an error if the staircase invariant is violated.
    pub fn try_new(code: LdpcCode) -> Result<Self, String> {
        let info_cols = code.info_cols();
        for entry in code.entries() {
            // Only check parity columns (base_col >= info_cols), skip the diagonal entry
            if entry.base_col >= info_cols && entry.base_col != info_cols + entry.base_row {
                // Off-diagonal parity entries must reference strictly earlier rows
                // (i.e., base_col < info_cols + base_row) for the staircase
                // back-substitution to work correctly.
                if entry.base_col > info_cols + entry.base_row {
                    return Err(format!(
                        "Staircase invariant violated: base_row={} has parity entry at base_col={} \
                         (expected <= {})",
                        entry.base_row,
                        entry.base_col,
                        info_cols + entry.base_row,
                    ));
                }
            }
        }
        Ok(Self { code })
    }

    /// Returns a reference to the underlying code.
    pub fn code(&self) -> &LdpcCode {
        &self.code
    }

    /// Encode a block of information bits into a systematic codeword.
    ///
    /// Input: `info_bits` of length `code.info_bits()`, each element 0 or 1.
    /// Output: codeword of length `code.coded_bits()` = [info_bits | parity_bits].
    ///
    /// # Panics
    /// Panics if `info_bits.len() != code.info_bits()`.
    pub fn encode_block(&self, info_bits: &[u8]) -> Vec<u8> {
        let k = self.code.info_bits();
        let n = self.code.coded_bits();
        assert_eq!(
            info_bits.len(),
            k,
            "Expected {} info bits, got {}",
            k,
            info_bits.len()
        );

        self.encode_block_inner(info_bits, k, n)
    }

    /// Encode a block, returning an error on length mismatch instead of panicking.
    pub fn try_encode_block(&self, info_bits: &[u8]) -> Result<Vec<u8>, String> {
        let k = self.code.info_bits();
        let n = self.code.coded_bits();
        if info_bits.len() != k {
            return Err(format!("Expected {} info bits, got {}", k, info_bits.len()));
        }
        Ok(self.encode_block_inner(info_bits, k, n))
    }

    fn encode_block_inner(&self, info_bits: &[u8], k: usize, n: usize) -> Vec<u8> {
        // Build the codeword: start with info bits, then compute parity
        let mut codeword = vec![0u8; n];
        codeword[..k].copy_from_slice(info_bits);

        // Compute parity bits using the staircase structure of the parity sub-matrix.
        // For each check row i, compute syndrome from info bits and previously
        // computed parity bits, then set the diagonal parity bit to satisfy the check.
        let z = LIFTING_FACTOR;
        let info_cols = self.code.info_cols();
        let n_check_rows = self.code.base_rows();

        // For each base row, process in order (staircase allows sequential resolution)
        for base_row in 0..n_check_rows {
            // For each sub-row within this base row's Z x Z block
            for sub_row in 0..z {
                let mut syndrome = 0u8;

                // Accumulate syndrome from all entries in this base row
                for entry in self.code.entries() {
                    if entry.base_row != base_row {
                        continue;
                    }

                    // Skip the diagonal parity entry (we're solving for it)
                    if entry.base_col == info_cols + base_row {
                        continue;
                    }

                    let col = entry.base_col * z + (sub_row + entry.shift) % z;
                    syndrome ^= codeword[col];
                }

                // The diagonal entry has shift=0, so parity bit position is straightforward
                let parity_pos = (info_cols + base_row) * z + sub_row;
                codeword[parity_pos] = syndrome;
            }
        }

        debug_assert!(
            self.code.check_codeword(&codeword),
            "Encoder produced invalid codeword"
        );

        codeword
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fec::ldpc::codes::CodeRate;

    #[test]
    fn test_encode_all_zeros() {
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
            let info = vec![0u8; code.info_bits()];
            let cw = enc.encode_block(&info);

            // All-zero info should produce all-zero codeword
            assert!(
                cw.iter().all(|&b| b == 0),
                "{:?}: all-zero encode failed",
                rate
            );
            assert!(
                code.check_codeword(&cw),
                "{:?}: codeword check failed",
                rate
            );
        }
    }

    #[test]
    fn test_encode_output_length() {
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
            let info: Vec<u8> = (0..code.info_bits()).map(|i| (i % 2) as u8).collect();
            let cw = enc.encode_block(&info);
            assert_eq!(cw.len(), 1944, "{:?}: wrong output length", rate);
        }
    }

    #[test]
    fn test_systematic_property() {
        // Info bits should appear unchanged at the start of the codeword
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
            let info: Vec<u8> = (0..code.info_bits())
                .map(|i| ((i * 7 + 3) % 2) as u8)
                .collect();
            let cw = enc.encode_block(&info);
            assert_eq!(
                &cw[..code.info_bits()],
                &info[..],
                "{:?}: systematic property violated",
                rate
            );
        }
    }

    #[test]
    fn test_valid_codewords() {
        // Every encoded block should satisfy H * c = 0
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

            // Test several patterns
            for seed in 0..5u64 {
                let info: Vec<u8> = (0..code.info_bits())
                    .map(|i| (((i as u64).wrapping_mul(seed.wrapping_add(7)) + 13) % 2) as u8)
                    .collect();
                let cw = enc.encode_block(&info);
                assert!(
                    code.check_codeword(&cw),
                    "{:?} seed {}: codeword failed parity check",
                    rate,
                    seed
                );
            }
        }
    }

    #[test]
    fn test_encode_deterministic() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code.clone());
        let info: Vec<u8> = (0..code.info_bits()).map(|i| (i % 3 == 0) as u8).collect();
        let cw1 = enc.encode_block(&info);
        let cw2 = enc.encode_block(&info);
        assert_eq!(cw1, cw2, "Encoding should be deterministic");
    }

    #[test]
    fn test_different_inputs_different_codewords() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code.clone());
        let info1 = vec![0u8; code.info_bits()];
        let mut info2 = vec![0u8; code.info_bits()];
        info2[0] = 1;
        let cw1 = enc.encode_block(&info1);
        let cw2 = enc.encode_block(&info2);
        assert_ne!(
            cw1, cw2,
            "Different inputs should produce different codewords"
        );
    }

    #[test]
    #[should_panic(expected = "Expected")]
    fn test_wrong_input_length() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let enc = LdpcEncoder::new(code);
        enc.encode_block(&[0, 1, 0]); // Too short
    }
}
