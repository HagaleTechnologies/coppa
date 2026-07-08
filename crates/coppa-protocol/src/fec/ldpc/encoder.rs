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

// =========================================================================
// NR BG2 mother-code encoder (Task 4)
// =========================================================================
//
// Generic (table-driven) two-step systematic encoder for the NR BG2 mother
// code (`super::nr_bg2`, Zc=176, KB=10 info columns, 42 parity check rows).
// Implemented purely from the base graph's sparse (row, col, shift) entries
// -- no hand-specialized row indices -- per this task's brief:
//
//   "BG2's parity has a 4-column dual-diagonal core (one weight-3 column) +
//    extension columns; the standard two-step encoder is: (1) sum all
//    base-row equations to isolate the weight-3 column's first block, (2)
//    back-substitute the remaining core parities, (3) extension parities
//    are then explicit single-row sums."
//
// Concretely: (1)+(2) are done here via a *generic* GF(2) Gauss-Jordan
// inversion of the lifted 4*Zc x 4*Zc core-parity submatrix (rather than a
// closed-form formula assuming a specific structure), so correctness does
// not depend on any assumption about which entries make up the "weight-3
// column" -- it falls out of whatever the transcribed table actually
// contains, and is checked directly by `H * c^T = 0` in this module's tests
// and in `tools/gen_nr_bg2`'s independent round-trip validator. (3) is a
// direct per-row XOR sum, since every extension column has degree exactly 1
// (see `nr_bg2` provenance docs) -- each extension parity bit is the unique
// unknown in its own row's equation.
use super::nr_bg2;

/// GF(2) matrix stored as bit-packed rows (`u64` words). Only used for the
/// small (`CORE_PARITY_COLS * Zc` x same, i.e. 704x704 for this codec's
/// fixed Zc=176) core-parity submatrix inversion -- everything else in the
/// encoder is sparse and handled by [`lifted_block_matvec`] directly.
#[derive(Debug, Clone)]
struct Gf2Matrix {
    rows: usize,
    cols: usize,
    words_per_row: usize,
    data: Vec<u64>,
}

impl Gf2Matrix {
    fn zeros(rows: usize, cols: usize) -> Self {
        let words_per_row = cols.div_ceil(64);
        Self {
            rows,
            cols,
            words_per_row,
            data: vec![0u64; rows * words_per_row],
        }
    }

    #[inline]
    fn word_bit(col: usize) -> (usize, u64) {
        (col / 64, 1u64 << (col % 64))
    }

    fn get(&self, r: usize, c: usize) -> bool {
        let (w, bit) = Self::word_bit(c);
        self.data[r * self.words_per_row + w] & bit != 0
    }

    fn set(&mut self, r: usize, c: usize, v: bool) {
        let (w, bit) = Self::word_bit(c);
        let idx = r * self.words_per_row + w;
        if v {
            self.data[idx] |= bit;
        } else {
            self.data[idx] &= !bit;
        }
    }

    fn xor_row_into(&mut self, dst: usize, src: usize) {
        let wpr = self.words_per_row;
        for w in 0..wpr {
            self.data[dst * wpr + w] ^= self.data[src * wpr + w];
        }
    }

    /// Gauss-Jordan inversion over GF(2). Returns `None` if singular (should
    /// never happen for a genuine NR base graph's core submatrix -- treated
    /// as a construction-time invariant violation by the caller).
    fn invert(&self) -> Option<Gf2Matrix> {
        assert_eq!(self.rows, self.cols, "invert requires a square matrix");
        let n = self.rows;
        let mut work = Gf2Matrix::zeros(n, n);
        work.data.copy_from_slice(&self.data);
        let mut inv = Gf2Matrix::zeros(n, n);
        for i in 0..n {
            inv.set(i, i, true);
        }

        for col in 0..n {
            let pivot = (col..n).find(|&r| work.get(r, col))?;
            if pivot != col {
                for w in 0..work.words_per_row {
                    work.data
                        .swap(col * work.words_per_row + w, pivot * work.words_per_row + w);
                    inv.data
                        .swap(col * inv.words_per_row + w, pivot * inv.words_per_row + w);
                }
            }
            for r in 0..n {
                if r != col && work.get(r, col) {
                    work.xor_row_into(r, col);
                    inv.xor_row_into(r, col);
                }
            }
        }

        Some(inv)
    }

    /// Dense GF(2) matrix-vector product: `self * x` (mod 2).
    fn mul_vec(&self, x: &[u8]) -> Vec<u8> {
        assert_eq!(x.len(), self.cols);
        let mut out = vec![0u8; self.rows];
        for (r, out_r) in out.iter_mut().enumerate() {
            let mut acc = 0u8;
            for (c, &xc) in x.iter().enumerate() {
                if self.get(r, c) {
                    acc ^= xc;
                }
            }
            *out_r = acc;
        }
        out
    }
}

/// Lifted block matvec: for base-graph edges filtered to
/// `row in [row_lo, row_lo+row_n)` and `col in [col_lo, col_lo+col_n)`,
/// XOR-accumulate `x[(col-col_lo)*Z + (i+shift)%Z]` into
/// `out[(row-row_lo)*Z + i]` for every `i` in `0..Z`. Generic over any
/// (row, col, shift) sparse table -- no hardcoded indices.
fn lifted_block_matvec(
    entries: &[(usize, usize, usize)],
    z: usize,
    row_lo: usize,
    row_n: usize,
    col_lo: usize,
    col_n: usize,
    x: &[u8],
) -> Vec<u8> {
    let mut out = vec![0u8; row_n * z];
    for &(r, c, s) in entries {
        if r < row_lo || r >= row_lo + row_n {
            continue;
        }
        if c < col_lo || c >= col_lo + col_n {
            continue;
        }
        let rr = r - row_lo;
        let cc = c - col_lo;
        for i in 0..z {
            out[rr * z + i] ^= x[cc * z + (i + s) % z];
        }
    }
    out
}

/// Build the dense lifted core-parity matrix: base rows
/// `0..CORE_PARITY_COLS`, base cols `KB..KB+CORE_PARITY_COLS`, each lifted to
/// a `Zc x Zc` circulant-shift block.
fn build_core_matrix(entries: &[(usize, usize, usize)], z: usize) -> Gf2Matrix {
    let core = nr_bg2::CORE_PARITY_COLS;
    let kb = nr_bg2::KB;
    let n = core * z;
    let mut m = Gf2Matrix::zeros(n, n);
    for &(r, c, s) in entries {
        if r >= core || c < kb || c >= kb + core {
            continue;
        }
        for i in 0..z {
            m.set(r * z + i, (c - kb) * z + (i + s) % z, true);
        }
    }
    m
}

/// Systematic encoder for the NR BG2 mother code. Caches the (small, 704x704
/// for Zc=176) core-parity inverse at construction so `encode_mother` is a
/// handful of sparse matvecs plus one dense 704x704 GF(2) matvec per call.
#[derive(Debug, Clone)]
pub struct NrBg2Encoder {
    core_inv: Gf2Matrix,
}

impl Default for NrBg2Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl NrBg2Encoder {
    pub fn new() -> Self {
        let core = build_core_matrix(nr_bg2::ENTRIES, nr_bg2::ZC);
        let core_inv = core
            .invert()
            .expect("BG2 core parity submatrix must be invertible over GF(2)");
        Self { core_inv }
    }

    /// Encode `info` (length `KB*ZC` = 1760) into the 8800-bit mother
    /// codeword: `[info[PUNCTURED_INFO_COLS*ZC..], parity]` (the first
    /// `PUNCTURED_INFO_COLS*ZC` = 352 systematic bits are never transmitted,
    /// standard NR puncturing -- see `rate_match` module docs).
    ///
    /// # Panics
    /// Panics if `info.len() != KB*ZC`.
    pub fn encode_mother(&self, info: &[u8]) -> Vec<u8> {
        let z = nr_bg2::ZC;
        let kb = nr_bg2::KB;
        let core = nr_bg2::CORE_PARITY_COLS;
        let base_rows = nr_bg2::BASE_ROWS;
        let punctured = nr_bg2::PUNCTURED_INFO_COLS;

        assert_eq!(
            info.len(),
            kb * z,
            "expected {} info bits, got {}",
            kb * z,
            info.len()
        );

        // rhs_a = hm_a * info  (hm_a: base rows 0..CORE_PARITY_COLS, cols 0..KB)
        let rhs_a = lifted_block_matvec(nr_bg2::ENTRIES, z, 0, core, 0, kb, info);
        let p_a = self.core_inv.mul_vec(&rhs_a);

        // p_b[row] = (hm_c1 * info)[row] xor (hm_c2 * p_a)[row], explicit
        // per-row sums (every extension column has degree exactly 1).
        let ext_rows = base_rows - core;
        let c1 = lifted_block_matvec(nr_bg2::ENTRIES, z, core, ext_rows, 0, kb, info);
        let c2 = lifted_block_matvec(nr_bg2::ENTRIES, z, core, ext_rows, kb, core, &p_a);
        let p_b: Vec<u8> = c1.iter().zip(c2.iter()).map(|(&a, &b)| a ^ b).collect();

        let mut mother = Vec::with_capacity((nr_bg2::BASE_COLS - punctured) * z);
        mother.extend_from_slice(&info[punctured * z..]);
        mother.extend_from_slice(&p_a);
        mother.extend_from_slice(&p_b);
        mother
    }
}

#[cfg(test)]
mod nr_bg2_encoder_tests {
    use super::*;

    fn full_codeword_from_info(info: &[u8], mother: &[u8]) -> Vec<u8> {
        let z = nr_bg2::ZC;
        let punctured = nr_bg2::PUNCTURED_INFO_COLS;
        let mut full = Vec::with_capacity(nr_bg2::BASE_COLS * z);
        full.extend_from_slice(&info[..punctured * z]);
        full.extend_from_slice(mother);
        full
    }

    fn check_full_codeword(full: &[u8]) -> bool {
        let z = nr_bg2::ZC;
        for row in 0..nr_bg2::BASE_ROWS {
            for i in 0..z {
                let mut syn = 0u8;
                for &(r, c, s) in nr_bg2::ENTRIES {
                    if r != row {
                        continue;
                    }
                    syn ^= full[c * z + (i + s) % z];
                }
                if syn != 0 {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn mother_length_is_8800() {
        let enc = NrBg2Encoder::new();
        let info = vec![0u8; nr_bg2::KB * nr_bg2::ZC];
        let mother = enc.encode_mother(&info);
        assert_eq!(
            mother.len(),
            (nr_bg2::BASE_COLS - nr_bg2::PUNCTURED_INFO_COLS) * nr_bg2::ZC
        );
    }

    #[test]
    fn all_zero_info_gives_all_zero_mother() {
        let enc = NrBg2Encoder::new();
        let info = vec![0u8; nr_bg2::KB * nr_bg2::ZC];
        let mother = enc.encode_mother(&info);
        assert!(mother.iter().all(|&b| b == 0));
    }

    #[test]
    fn random_info_produces_valid_codewords() {
        let enc = NrBg2Encoder::new();
        let mut seed: u64 = 0xC0FF_EE12_3456_789A;
        let mut next_bit = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed & 1) as u8
        };
        for _trial in 0..12 {
            let info: Vec<u8> = (0..nr_bg2::KB * nr_bg2::ZC).map(|_| next_bit()).collect();
            let mother = enc.encode_mother(&info);
            let full = full_codeword_from_info(&info, &mother);
            assert!(
                check_full_codeword(&full),
                "encoded codeword failed H*c^T=0"
            );
        }
    }

    #[test]
    fn systematic_property_holds_for_non_punctured_info() {
        let enc = NrBg2Encoder::new();
        let info: Vec<u8> = (0..nr_bg2::KB * nr_bg2::ZC)
            .map(|i| (i % 2) as u8)
            .collect();
        let mother = enc.encode_mother(&info);
        let non_punctured_len = (nr_bg2::KB - nr_bg2::PUNCTURED_INFO_COLS) * nr_bg2::ZC;
        assert_eq!(
            &mother[..non_punctured_len],
            &info[nr_bg2::PUNCTURED_INFO_COLS * nr_bg2::ZC..]
        );
    }

    #[test]
    fn encode_is_deterministic() {
        let enc = NrBg2Encoder::new();
        let info: Vec<u8> = (0..nr_bg2::KB * nr_bg2::ZC)
            .map(|i| ((i * 3 + 1) % 2) as u8)
            .collect();
        assert_eq!(enc.encode_mother(&info), enc.encode_mother(&info));
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
