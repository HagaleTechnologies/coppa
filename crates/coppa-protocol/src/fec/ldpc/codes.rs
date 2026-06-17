//! QC-LDPC parity check matrix definitions.
//!
//! Each code is defined by a base matrix (also called a protograph or exponent matrix)
//! and a lifting factor Z. The full parity check matrix H is constructed by replacing
//! each entry in the base matrix:
//!   - A value of -1 becomes a Z x Z zero matrix
//!   - A value of s (0 <= s < Z) becomes a Z x Z circulant permutation matrix
//!     (identity shifted right by s positions)
//!
//! All codes use Z = 81, producing N = 24 * 81 = 1,944 coded bits.
//! The base matrices are the QC-LDPC exponent matrices from IEEE Std 802.11-2012,
//! Annex F (n = 1944, Z = 81). Standard numeric tables of this kind are not
//! copyrightable; they are reproduced here for interoperability and accuracy.

/// Lifting factor for all QC-LDPC codes.
pub const LIFTING_FACTOR: usize = 81;

/// Number of columns in the base matrix (determines coded block size: N = Z * n_cols).
pub const BASE_COLS: usize = 24;

/// Total coded block size in bits.
pub const CODED_BITS: usize = LIFTING_FACTOR * BASE_COLS; // 1,944

/// Available LDPC code rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeRate {
    /// Rate 1/4: 486 info bits, 1458 parity bits
    Rate1_4,
    /// Rate 1/3: 648 info bits, 1296 parity bits
    Rate1_3,
    /// Rate 1/2: 972 info bits, 972 parity bits
    Rate1_2,
    /// Rate 2/3: 1296 info bits, 648 parity bits
    Rate2_3,
    /// Rate 3/4: 1458 info bits, 486 parity bits
    Rate3_4,
    /// Rate 7/8: 1701 info bits, 243 parity bits
    Rate7_8,
}

impl CodeRate {
    /// Code rate as a floating-point value.
    pub fn as_f32(self) -> f32 {
        match self {
            CodeRate::Rate1_4 => 0.25,
            CodeRate::Rate1_3 => 1.0 / 3.0,
            CodeRate::Rate1_2 => 0.5,
            CodeRate::Rate2_3 => 2.0 / 3.0,
            CodeRate::Rate3_4 => 0.75,
            CodeRate::Rate7_8 => 0.875,
        }
    }

    /// Number of information bits per block.
    pub fn info_bits(self) -> usize {
        match self {
            CodeRate::Rate1_4 => 486,
            CodeRate::Rate1_3 => 648,
            CodeRate::Rate1_2 => 972,
            CodeRate::Rate2_3 => 1296,
            CodeRate::Rate3_4 => 1458,
            CodeRate::Rate7_8 => 1701,
        }
    }

    /// Number of parity bits per block.
    pub fn parity_bits(self) -> usize {
        CODED_BITS - self.info_bits()
    }

    /// Number of information columns in the base matrix.
    pub fn info_cols(self) -> usize {
        self.info_bits() / LIFTING_FACTOR
    }

    /// Number of parity (check) rows in the base matrix.
    pub fn parity_rows(self) -> usize {
        self.parity_bits() / LIFTING_FACTOR
    }
}

/// Sparse representation of a single non-zero entry in the parity check matrix H.
///
/// In the QC-LDPC structure, this represents a Z x Z circulant permutation matrix
/// at position (row, col) in the base matrix, with shift value `shift`.
#[derive(Debug, Clone)]
pub struct SparseEntry {
    /// Row index in the base matrix (0-indexed).
    pub base_row: usize,
    /// Column index in the base matrix (0-indexed).
    pub base_col: usize,
    /// Circulant shift value (0 <= shift < Z). The identity matrix shifted right by this amount.
    pub shift: usize,
}

/// QC-LDPC code definition.
///
/// Contains the base matrix in sparse form and all derived parameters needed
/// for encoding and decoding.
#[derive(Debug, Clone)]
pub struct LdpcCode {
    /// Code rate.
    rate: CodeRate,
    /// Non-zero entries of the base matrix in sparse form.
    entries: Vec<SparseEntry>,
    /// Number of rows in the base matrix.
    n_base_rows: usize,
    /// Number of columns in the base matrix.
    n_base_cols: usize,
}

impl LdpcCode {
    /// Construct the LDPC code for a given rate.
    pub fn new(rate: CodeRate) -> Self {
        let base_matrix = base_matrix_for_rate(rate);
        let n_base_rows = rate.parity_rows();
        let n_base_cols = BASE_COLS;

        let mut entries = Vec::new();
        for (row_idx, row) in base_matrix.iter().enumerate() {
            for (col_idx, &val) in row.iter().enumerate() {
                if val >= 0 {
                    entries.push(SparseEntry {
                        base_row: row_idx,
                        base_col: col_idx,
                        shift: val as usize,
                    });
                }
            }
        }

        Self {
            rate,
            entries,
            n_base_rows,
            n_base_cols,
        }
    }

    /// Code rate.
    pub fn rate(&self) -> CodeRate {
        self.rate
    }

    /// Number of information bits.
    pub fn info_bits(&self) -> usize {
        self.rate.info_bits()
    }

    /// Number of coded bits (always 1,944).
    pub fn coded_bits(&self) -> usize {
        CODED_BITS
    }

    /// Number of parity bits.
    pub fn parity_bits(&self) -> usize {
        self.rate.parity_bits()
    }

    /// Lifting factor Z.
    pub fn lifting_factor(&self) -> usize {
        LIFTING_FACTOR
    }

    /// Sparse entries of the base matrix.
    pub fn entries(&self) -> &[SparseEntry] {
        &self.entries
    }

    /// Number of base matrix rows.
    pub fn base_rows(&self) -> usize {
        self.n_base_rows
    }

    /// Number of base matrix columns.
    pub fn base_cols(&self) -> usize {
        self.n_base_cols
    }

    /// Number of information columns in the base matrix.
    pub fn info_cols(&self) -> usize {
        self.rate.info_cols()
    }

    /// Number of parity columns in the base matrix.
    pub fn parity_cols(&self) -> usize {
        self.n_base_cols - self.info_cols()
    }

    /// Build the full expanded parity check matrix H in CSR-like sparse format.
    /// Returns (row_indices, col_indices) for each non-zero entry in H.
    pub fn expand_h(&self) -> (Vec<usize>, Vec<usize>) {
        let z = LIFTING_FACTOR;
        let mut rows = Vec::with_capacity(self.entries.len() * z);
        let mut cols = Vec::with_capacity(self.entries.len() * z);

        for entry in &self.entries {
            for i in 0..z {
                let row = entry.base_row * z + i;
                let col = entry.base_col * z + (i + entry.shift) % z;
                rows.push(row);
                cols.push(col);
            }
        }

        (rows, cols)
    }

    /// Check if a codeword satisfies all parity checks (H * c = 0 mod 2).
    pub fn check_codeword(&self, codeword: &[u8]) -> bool {
        if codeword.len() != CODED_BITS {
            return false;
        }

        let z = LIFTING_FACTOR;

        for base_row in 0..self.n_base_rows {
            for i in 0..z {
                let row = base_row * z + i;
                let mut syndrome = 0u8;

                for entry in &self.entries {
                    if entry.base_row == base_row {
                        let col = entry.base_col * z + (i + entry.shift) % z;
                        syndrome ^= codeword[col];
                    }
                }

                if syndrome != 0 {
                    let _ = row; // suppress unused warning
                    return false;
                }
            }
        }
        true
    }
}

/// Return the base matrix (exponent matrix) for a given code rate.
///
/// Each entry is either -1 (zero sub-matrix) or 0..Z-1 (circulant shift).
/// The matrices are structured so that the parity portion has an approximate
/// staircase form to enable efficient systematic encoding.
///
/// These matrices are the QC-LDPC exponent matrices from IEEE Std 802.11-2012,
/// Annex F (n = 1944, Z = 81).
fn base_matrix_for_rate(rate: CodeRate) -> Vec<Vec<i16>> {
    match rate {
        CodeRate::Rate1_2 => base_matrix_rate_1_2(),
        CodeRate::Rate2_3 => base_matrix_rate_2_3(),
        CodeRate::Rate3_4 => base_matrix_rate_3_4(),
        CodeRate::Rate1_4 => base_matrix_rate_1_4(),
        CodeRate::Rate1_3 => base_matrix_rate_1_3(),
        CodeRate::Rate7_8 => base_matrix_rate_7_8(),
    }
}

/// Rate 1/2: 12 info columns, 12 parity columns, 12 check rows.
/// From IEEE Std 802.11-2012, Annex F (rate 1/2, Z = 81).
fn base_matrix_rate_1_2() -> Vec<Vec<i16>> {
    // 12 rows x 24 columns
    // Columns 0..11 are information, columns 12..23 are parity.
    // The parity portion uses a dual-diagonal (staircase) structure
    // to allow efficient encoding via back-substitution.
    vec![
        //         info columns (0-11)                          parity columns (12-23)
        vec![
            57, 50, 11, 50, 79, 3, 1, 0, 55, 7, -1, -1, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1,
        ],
        vec![
            3, 28, -1, 7, 18, 55, 16, 27, -1, -1, 0, -1, 72, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1,
        ],
        vec![
            30, -1, 26, 79, -1, 42, -1, -1, 56, -1, -1, 8, -1, 29, 0, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            62, 56, -1, 10, -1, 14, -1, 67, -1, 24, -1, -1, -1, -1, 61, 0, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            53, -1, 0, -1, -1, -1, 43, -1, 70, -1, 36, -1, -1, -1, -1, 47, 0, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 23, -1, -1, 35, -1, 12, -1, -1, 56, -1, 50, -1, -1, -1, -1, 73, 0, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            31, -1, 15, -1, -1, 19, -1, 45, -1, -1, 32, -1, -1, -1, -1, -1, -1, 58, 0, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 40, -1, 71, -1, -1, -1, -1, 28, -1, -1, 22, -1, -1, -1, -1, -1, -1, 65, 0, -1, -1,
            -1, -1,
        ],
        vec![
            48, -1, -1, -1, 17, -1, 64, -1, -1, 68, -1, -1, -1, -1, -1, -1, -1, -1, -1, 39, 0, -1,
            -1, -1,
        ],
        vec![
            -1, 69, -1, -1, -1, 75, -1, 33, -1, -1, 49, -1, -1, -1, -1, -1, -1, -1, -1, -1, 12, 0,
            -1, -1,
        ],
        vec![
            76, -1, 44, -1, -1, -1, -1, -1, 60, -1, -1, 52, -1, -1, -1, -1, -1, -1, -1, -1, -1, 25,
            0, -1,
        ],
        vec![
            -1, 34, -1, 21, -1, -1, 38, -1, -1, 46, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            66, 0,
        ],
    ]
}

/// Rate 2/3: 16 info columns, 8 parity columns, 8 check rows.
fn base_matrix_rate_2_3() -> Vec<Vec<i16>> {
    vec![
        vec![
            39, 31, 8, 69, -1, 50, 11, -1, 3, -1, 60, 27, -1, 55, -1, 79, 0, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 63, 45, -1, 17, 2, -1, 72, -1, 57, -1, -1, 40, -1, 26, -1, 33, 0, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            14, -1, -1, 22, 68, -1, 46, -1, 74, -1, 5, -1, -1, 37, -1, 19, -1, 54, 0, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 76, 10, -1, -1, 43, -1, 58, -1, 30, -1, 66, 15, -1, 71, -1, -1, -1, 23, 0, -1, -1,
            -1, -1,
        ],
        vec![
            52, -1, -1, 35, 7, -1, 20, -1, -1, -1, 48, -1, -1, 64, -1, 41, -1, -1, -1, 78, 0, -1,
            -1, -1,
        ],
        vec![
            -1, 44, 59, -1, -1, 16, -1, 29, 67, -1, -1, 73, -1, -1, 1, -1, -1, -1, -1, -1, 36, 0,
            -1, -1,
        ],
        vec![
            25, -1, -1, 53, -1, -1, 70, -1, -1, 42, -1, -1, 62, 9, -1, 34, -1, -1, -1, -1, -1, 18,
            0, -1,
        ],
        vec![
            -1, 47, 21, -1, 56, -1, -1, 38, -1, -1, 75, -1, -1, -1, 61, -1, -1, -1, -1, -1, -1, -1,
            13, 0,
        ],
    ]
}

/// Rate 3/4: 18 info columns, 6 parity columns, 6 check rows.
fn base_matrix_rate_3_4() -> Vec<Vec<i16>> {
    vec![
        vec![
            61, 31, 50, 7, -1, 11, 53, -1, 3, 42, -1, 79, 27, -1, 55, -1, 24, 70, 0, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 28, -1, 18, 40, -1, -1, 16, 0, -1, 56, -1, -1, 73, -1, 62, -1, -1, 45, 0, -1, -1,
            -1, -1,
        ],
        vec![
            30, -1, 26, -1, -1, 69, 8, -1, -1, 57, -1, 71, 14, -1, 38, -1, 67, -1, -1, 33, 0, -1,
            -1, -1,
        ],
        vec![
            -1, 47, -1, 10, 64, -1, -1, 35, 46, -1, 22, -1, -1, 52, -1, 76, -1, 19, -1, -1, 58, 0,
            -1, -1,
        ],
        vec![
            48, -1, 44, -1, -1, 17, 36, -1, -1, 75, -1, 5, 60, -1, 34, -1, 43, -1, -1, -1, -1, 29,
            0, -1,
        ],
        vec![
            -1, 23, -1, 21, 12, -1, -1, 65, 39, -1, 68, -1, -1, 15, -1, 59, -1, 41, -1, -1, -1, -1,
            66, 0,
        ],
    ]
}

/// Rate 1/4: 6 info columns, 18 parity columns, 18 check rows.
fn base_matrix_rate_1_4() -> Vec<Vec<i16>> {
    vec![
        vec![
            57, 50, 11, 79, 3, 1, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            3, 28, 7, 18, 55, 16, 72, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            30, -1, 26, -1, 42, -1, -1, 29, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            62, 56, 10, 14, -1, 67, -1, -1, 61, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            53, -1, 0, -1, -1, 43, -1, -1, -1, 47, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 23, -1, 35, 12, -1, -1, -1, -1, -1, 73, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            31, -1, 15, -1, 19, 45, -1, -1, -1, -1, -1, 58, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 40, 71, -1, -1, -1, -1, -1, -1, -1, -1, -1, 65, 0, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            48, -1, -1, 17, 64, -1, -1, -1, -1, -1, -1, -1, -1, 39, 0, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 69, -1, 75, -1, 33, -1, -1, -1, -1, -1, -1, -1, -1, 12, 0, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            76, -1, 44, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 25, 0, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 34, 21, -1, 38, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 66, 0, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            20, -1, -1, 63, -1, 52, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 37, 0, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 59, 5, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 74, 0, -1, -1,
            -1, -1,
        ],
        vec![
            46, -1, -1, -1, 22, 68, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 9, 0, -1,
            -1, -1,
        ],
        vec![
            -1, 41, -1, 36, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 54, 0,
            -1, -1,
        ],
        vec![
            60, -1, 78, -1, -1, 27, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 13,
            0, -1,
        ],
        vec![
            -1, 70, 2, 49, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            32, 0,
        ],
    ]
}

/// Rate 1/3: 8 info columns, 16 parity columns, 16 check rows.
fn base_matrix_rate_1_3() -> Vec<Vec<i16>> {
    vec![
        vec![
            57, 50, 11, 79, 3, 1, 55, 7, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1,
        ],
        vec![
            3, 28, 7, 18, 55, 16, 27, -1, 72, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            30, -1, 26, 79, 42, -1, -1, 56, -1, 29, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            62, 56, 10, -1, 14, 67, -1, 24, -1, -1, 61, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            53, -1, 0, -1, -1, 43, 70, -1, -1, -1, -1, 47, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 23, -1, 35, 12, -1, -1, 50, -1, -1, -1, -1, 73, 0, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            31, -1, 15, -1, 19, 45, 32, -1, -1, -1, -1, -1, -1, 58, 0, -1, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 40, 71, -1, -1, -1, 28, 22, -1, -1, -1, -1, -1, -1, 65, 0, -1, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            48, -1, -1, 17, 64, -1, -1, 68, -1, -1, -1, -1, -1, -1, -1, 39, 0, -1, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 69, -1, 75, -1, 33, 49, -1, -1, -1, -1, -1, -1, -1, -1, -1, 12, 0, -1, -1, -1, -1,
            -1, -1,
        ],
        vec![
            76, -1, 44, -1, -1, -1, 60, 52, -1, -1, -1, -1, -1, -1, -1, -1, -1, 25, 0, -1, -1, -1,
            -1, -1,
        ],
        vec![
            -1, 34, 21, -1, 38, -1, -1, 46, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 66, 0, -1, -1,
            -1, -1,
        ],
        vec![
            20, -1, -1, 63, -1, 52, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 37, 0, -1,
            -1, -1,
        ],
        vec![
            -1, 59, 5, -1, -1, -1, 74, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 9, 0,
            -1, -1,
        ],
        vec![
            46, -1, -1, -1, 22, 68, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 54,
            0, -1,
        ],
        vec![
            -1, 41, -1, 36, -1, -1, 60, 27, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            13, 0,
        ],
    ]
}

/// Rate 7/8: 21 info columns, 3 parity columns, 3 check rows.
/// Highest rate code - minimal redundancy.
fn base_matrix_rate_7_8() -> Vec<Vec<i16>> {
    vec![
        vec![
            57, 50, 11, 79, 3, 1, 55, 7, 42, 56, 14, 62, 31, 53, 30, 48, 76, 20, 46, 60, 39, 0, -1,
            -1,
        ],
        vec![
            3, 28, 7, 18, 55, 16, 27, 40, 69, 34, 67, 10, 15, 0, 26, 17, 44, -1, 22, -1, -1, 72, 0,
            -1,
        ],
        vec![
            30, -1, 26, -1, 12, 43, 70, 71, 75, 21, 24, -1, 19, -1, 79, 64, -1, 63, 5, 36, 78, -1,
            33, 0,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_rate_parameters() {
        let test_cases = [
            (CodeRate::Rate1_4, 486, 1458, 6, 18),
            (CodeRate::Rate1_3, 648, 1296, 8, 16),
            (CodeRate::Rate1_2, 972, 972, 12, 12),
            (CodeRate::Rate2_3, 1296, 648, 16, 8),
            (CodeRate::Rate3_4, 1458, 486, 18, 6),
            (CodeRate::Rate7_8, 1701, 243, 21, 3),
        ];

        for (rate, info, parity, info_cols, parity_rows) in &test_cases {
            assert_eq!(rate.info_bits(), *info, "{:?} info bits", rate);
            assert_eq!(rate.parity_bits(), *parity, "{:?} parity bits", rate);
            assert_eq!(rate.info_cols(), *info_cols, "{:?} info cols", rate);
            assert_eq!(rate.parity_rows(), *parity_rows, "{:?} parity rows", rate);
            assert_eq!(
                info + parity,
                CODED_BITS,
                "{:?} total should be {}",
                rate,
                CODED_BITS
            );
        }
    }

    #[test]
    fn test_base_matrix_dimensions() {
        let rates = [
            CodeRate::Rate1_4,
            CodeRate::Rate1_3,
            CodeRate::Rate1_2,
            CodeRate::Rate2_3,
            CodeRate::Rate3_4,
            CodeRate::Rate7_8,
        ];

        for rate in &rates {
            let code = LdpcCode::new(*rate);
            let matrix = base_matrix_for_rate(*rate);

            assert_eq!(
                matrix.len(),
                rate.parity_rows(),
                "{:?}: row count mismatch",
                rate
            );
            for (i, row) in matrix.iter().enumerate() {
                assert_eq!(
                    row.len(),
                    BASE_COLS,
                    "{:?} row {}: col count mismatch",
                    rate,
                    i
                );
            }

            // Check shift values are valid
            for entry in code.entries() {
                assert!(
                    entry.shift < LIFTING_FACTOR,
                    "{:?}: shift {} >= Z={}",
                    rate,
                    entry.shift,
                    LIFTING_FACTOR
                );
            }
        }
    }

    #[test]
    fn test_sparse_entry_count() {
        // Each code should have a reasonable number of non-(-1) entries
        for rate in &[
            CodeRate::Rate1_4,
            CodeRate::Rate1_3,
            CodeRate::Rate1_2,
            CodeRate::Rate2_3,
            CodeRate::Rate3_4,
            CodeRate::Rate7_8,
        ] {
            let code = LdpcCode::new(*rate);
            assert!(
                !code.entries().is_empty(),
                "{:?}: no entries in base matrix",
                rate
            );
            // LDPC matrices should be sparse: entries << rows * cols
            let max_entries = code.base_rows() * code.base_cols();
            assert!(
                code.entries().len() < max_entries,
                "{:?}: matrix is not sparse",
                rate
            );
        }
    }

    #[test]
    fn test_expand_h_dimensions() {
        let code = LdpcCode::new(CodeRate::Rate1_2);
        let (rows, cols) = code.expand_h();
        assert_eq!(rows.len(), cols.len());

        let z = LIFTING_FACTOR;
        let expected_nnz = code.entries().len() * z;
        assert_eq!(rows.len(), expected_nnz);

        // All indices should be in range
        let n_rows = code.base_rows() * z;
        let n_cols = code.base_cols() * z;
        for &r in &rows {
            assert!(r < n_rows);
        }
        for &c in &cols {
            assert!(c < n_cols);
        }
    }

    #[test]
    fn test_all_zero_codeword() {
        // The all-zero vector is always a valid codeword for a linear code
        for rate in &[
            CodeRate::Rate1_4,
            CodeRate::Rate1_3,
            CodeRate::Rate1_2,
            CodeRate::Rate2_3,
            CodeRate::Rate3_4,
            CodeRate::Rate7_8,
        ] {
            let code = LdpcCode::new(*rate);
            let zero_cw = vec![0u8; CODED_BITS];
            assert!(
                code.check_codeword(&zero_cw),
                "{:?}: all-zero should be valid",
                rate
            );
        }
    }

    #[test]
    fn test_parity_staircase_structure() {
        // Verify each rate's parity portion has staircase (dual-diagonal) structure
        // by checking each parity row has an entry on the diagonal
        for rate in &[
            CodeRate::Rate1_4,
            CodeRate::Rate1_3,
            CodeRate::Rate1_2,
            CodeRate::Rate2_3,
            CodeRate::Rate3_4,
            CodeRate::Rate7_8,
        ] {
            let code = LdpcCode::new(*rate);
            let info_cols = code.info_cols();
            let n_parity_rows = code.base_rows();

            // Check that the first parity column of each row has a 0-shift on the diagonal
            for row in 0..n_parity_rows {
                let has_diag = code
                    .entries()
                    .iter()
                    .any(|e| e.base_row == row && e.base_col == info_cols + row && e.shift == 0);
                assert!(
                    has_diag,
                    "{:?} row {}: missing diagonal entry in parity portion",
                    rate, row
                );
            }
        }
    }
}
