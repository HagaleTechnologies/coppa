//! Generator + validator for the 3GPP TS 38.212 §5.3.2 NR LDPC "Base Graph 2"
//! (BG2), lifting-size family i_LS = 5 (Zc ∈ {11, 22, 44, 88, 176, 352}).
//!
//! Coppa's new mother code (Task 4 of the Phase 2 remediation roadmap) fixes
//! the lifting size at Zc = 176 for every speed level (one code for the whole
//! ladder, rate-matched down per level — see `rate_match.rs`), so this tool
//! only needs to transcribe, validate, and codegen the single Zc=176 shift
//! table, not the full 8-family/51-lifting-size standard.
//!
//! # Provenance
//!
//! The transcribed `ENTRIES` table below was cross-validated against **two
//! independent, actively-maintained, license-compatible open-source 5G PHY
//! implementations** that both implement 3GPP TS 38.212 Table 5.3.2-3 (BG2):
//!
//! 1. **NVIDIA Sionna** (Apache-2.0), `src/sionna/phy/fec/ldpc/codes/5G_bg2.csv`
//!    — a CSV transcription of the standard's BG2 table, columns 2..9 holding
//!    the raw `V_{i,j}` shift values for lifting-size-set indices `i_LS = 0..7`
//!    respectively (see that repo's `encoding.py::_load_basegraph`, which reads
//!    `bg_csv[r, i_ls + 2]`). Column index 7 (`i_ls = 5`) is the family
//!    containing Zc = 176.
//! 2. **srsRAN Project** (AGPL-3.0), an independent C++ implementation,
//!    `lib/phy/upper/channel_coding/ldpc/ldpc_luts_impl.cpp`, array
//!    `BG2_matrices[5]` (`NOF_LIFTING_INDICES = 8`, so index 5 is again the
//!    `i_LS = 5` family — confirmed against that repo's own
//!    `include/.../ldpc.h::all_lifting_sizes`/`LS176` mapping).
//!
//! Both sources were fetched fresh and diffed programmatically: **all 197
//! non-zero (row, col, shift) entries are identical between the two sources**
//! (byte-for-byte on the shift value, no `mod Zc` reduction needed — both
//! stores already give shift values in `[0, 176)` for this family). No entries
//! were adjusted, dropped, or "fixed" to make validators pass; this table is
//! copied verbatim from that cross-checked intersection.
//!
//! ## Correcting the task brief's `i_LS` guess
//!
//! The brief that commissioned this table guessed "`i_LS = 7`, `a = 11×2^4`"
//! for Zc = 176. That guess is wrong: 3GPP TS 38.212 Table 5.3.2-1 groups
//! lifting sizes by their odd factor `a` into 8 families, and the family
//! containing `a = 11` (`{11, 22, 44, 88, 176, 352}`) is index **5**, not 7
//! (index 7 is the `a = 15` family, `{15, 30, 60, 120, 240}`, which does not
//! even contain 176). This was confirmed directly from Sionna's own
//! `_sel_lifting` implementation (`s_val[5] == [11, 22, 44, 88, 176, 352]`),
//! not asserted from memory — see the Task 4 report for the fetched source
//! listing. All uses of `i_LS` in this tool and in the generated
//! `nr_bg2.rs` use the corrected value 5.
//!
//! ## Independent structural cross-check
//!
//! Beyond the two-source diff, the transcribed core submatrix (rows 0..4,
//! cols `KB..KB+4` = 10..14) was checked against the well-documented,
//! standard-mandated universal NR LDPC core structure (same for BG1 and BG2,
//! see e.g. Sionna's `_find_hm_b_inv` docstring, or Richardson & Kudekar,
//! "Design of Low-Density Parity Check Codes for 5G New Radio"):
//!
//! ```text
//! [ P_A  I    0    0  ]
//! [ 0    I    I    0  ]
//! [ P_B  0    I    I  ]
//! [ P_A  0    0    I  ]
//! ```
//!
//! where `P_A`/`P_B` are the (family-specific) shifts at base positions
//! `(0, KB)` and `(2, KB)`, and `I`/`0` are fixed-position shift-0
//! identity/absent blocks. The transcribed table matches this exactly
//! (`P_A = 0`, `P_B = 1`) — a third, independent (structural, not
//! source-comparison) consistency check that the transcription is correct.
//!
//! # Validators (decision 6)
//!
//! `main` runs, in order, and refuses to write `nr_bg2.rs` unless **all
//! four** pass:
//!
//! 1. **Dimensions**: 42 base rows × 52 base columns, every row and column
//!    has at least one entry, every shift is in `[0, Zc)`.
//! 2. **Zero 4-cycles at Zc = 176**: the lifted Tanner graph has no length-4
//!    cycles (the standard QC-LDPC girth-8-preserving check: no two base
//!    columns may share the same shift-difference in two different base
//!    rows).
//! 3. **Min column weight ≥ 2, scoped to the "core" columns** (the `KB = 10`
//!    info columns plus the 4 dual-diagonal parity columns, i.e. base
//!    columns 0..14). **This scoping is deliberate, not a validator
//!    weakened to force a pass**: NR LDPC "extension" parity columns
//!    (base columns 14..52 here) are *specified* to have degree exactly 1
//!    (each is the sole unknown in its own base row's parity equation —
//!    see this task's brief, "extension parities are then explicit
//!    single-row sums", and confirmed empirically below: every column
//!    14..52 in this transcription has weight exactly 1). A degree-2
//!    floor over the *full* 52-column graph would therefore reject any
//!    correctly-transcribed BG2 table, standards-compliant or not. Scoping
//!    to the core is what makes this validator meaningful: a real
//!    transcription defect (e.g. a dropped duplicate entry, a wrong row)
//!    is far more likely to show up as a spuriously-low-degree *info or
//!    core-parity* column, which is what this check actually guards
//!    against. The full per-column weight histogram is printed either way.
//! 4. **Encode/check round-trip `H·cᵀ = 0`**: builds the generic two-step
//!    systematic encoder (core dual-diagonal GF(2) solve + explicit
//!    single-row extension sums, driven entirely by the table above — see
//!    `gf2` and `encode_mother`, no hand-specialized row indices) and
//!    checks the produced codeword against every one of the 42×176 lifted
//!    parity checks, for several random information-bit patterns.

use std::fmt::Write as _;

/// Lifting factor. Fixed: this codec uses exactly one lifting size for the
/// whole speed-level ladder.
const ZC: usize = 176;
/// Number of base-graph check rows (= number of parity columns = 52 - 10).
const BASE_ROWS: usize = 42;
/// Number of base-graph columns (10 info + 42 parity), before the 2-column
/// info puncturing applied at the wire.
const BASE_COLS: usize = 52;
/// Number of information columns in the base graph ("K_b"), fixed at its
/// maximum (10) since this codec's info length is fixed at `KB * ZC = 1760`.
const KB: usize = 10;
/// Number of "core" dual-diagonal parity columns (fixed NR LDPC structural
/// constant, shared by BG1 and BG2).
const CORE_PARITY_COLS: usize = 4;
/// Number of Zc-blocks of systematic (info) columns punctured at the wire
/// (never transmitted, but part of the graph/encoder's algebra).
const PUNCTURED_INFO_COLS: usize = 2;

/// Transcribed (base_row, base_col, shift) entries. See module docs for
/// full provenance.
#[rustfmt::skip]
const ENTRIES: &[(usize, usize, usize)] = &[
    (0, 0, 156), (0, 1, 143), (0, 2, 14), (0, 3, 3),
    (0, 6, 40), (0, 9, 123), (0, 10, 0), (0, 11, 0),
    (1, 0, 17), (1, 3, 65), (1, 4, 63), (1, 5, 1),
    (1, 6, 55), (1, 7, 37), (1, 8, 171), (1, 9, 133),
    (1, 11, 0), (1, 12, 0), (2, 0, 98), (2, 1, 168),
    (2, 3, 107), (2, 4, 82), (2, 8, 142), (2, 10, 1),
    (2, 12, 0), (2, 13, 0), (3, 1, 53), (3, 2, 174),
    (3, 4, 174), (3, 5, 127), (3, 6, 17), (3, 7, 89),
    (3, 8, 17), (3, 9, 105), (3, 10, 0), (3, 13, 0),
    (4, 0, 86), (4, 1, 67), (4, 11, 83), (4, 14, 0),
    (5, 0, 79), (5, 1, 84), (5, 5, 35), (5, 7, 103),
    (5, 11, 60), (5, 15, 0), (6, 0, 47), (6, 5, 154),
    (6, 7, 10), (6, 9, 155), (6, 11, 29), (6, 16, 0),
    (7, 1, 48), (7, 5, 125), (7, 7, 24), (7, 11, 47),
    (7, 13, 55), (7, 17, 0), (8, 0, 53), (8, 1, 31),
    (8, 12, 161), (8, 18, 0), (9, 1, 104), (9, 8, 142),
    (9, 10, 99), (9, 11, 64), (9, 19, 0), (10, 0, 111),
    (10, 1, 25), (10, 6, 174), (10, 7, 23), (10, 20, 0),
    (11, 0, 91), (11, 7, 175), (11, 9, 24), (11, 13, 141),
    (11, 21, 0), (12, 1, 122), (12, 3, 11), (12, 11, 4),
    (12, 22, 0), (13, 0, 29), (13, 1, 91), (13, 8, 27),
    (13, 13, 127), (13, 23, 0), (14, 1, 11), (14, 6, 145),
    (14, 11, 8), (14, 13, 166), (14, 24, 0), (15, 0, 137),
    (15, 10, 103), (15, 11, 40), (15, 25, 0), (16, 1, 78),
    (16, 9, 158), (16, 11, 17), (16, 12, 165), (16, 26, 0),
    (17, 1, 134), (17, 5, 23), (17, 11, 62), (17, 12, 163),
    (17, 27, 0), (18, 0, 173), (18, 6, 31), (18, 7, 22),
    (18, 28, 0), (19, 0, 13), (19, 1, 135), (19, 10, 145),
    (19, 29, 0), (20, 1, 128), (20, 4, 52), (20, 11, 173),
    (20, 30, 0), (21, 0, 156), (21, 8, 166), (21, 13, 40),
    (21, 31, 0), (22, 1, 18), (22, 2, 163), (22, 32, 0),
    (23, 0, 110), (23, 3, 132), (23, 5, 150), (23, 33, 0),
    (24, 1, 113), (24, 2, 108), (24, 9, 61), (24, 34, 0),
    (25, 0, 72), (25, 5, 136), (25, 35, 0), (26, 2, 36),
    (26, 7, 38), (26, 12, 53), (26, 13, 145), (26, 36, 0),
    (27, 0, 42), (27, 6, 104), (27, 37, 0), (28, 1, 64),
    (28, 2, 24), (28, 5, 149), (28, 38, 0), (29, 0, 139),
    (29, 4, 161), (29, 39, 0), (30, 2, 84), (30, 5, 173),
    (30, 7, 93), (30, 9, 29), (30, 40, 0), (31, 1, 117),
    (31, 13, 148), (31, 41, 0), (32, 0, 116), (32, 5, 73),
    (32, 12, 142), (32, 42, 0), (33, 2, 105), (33, 7, 137),
    (33, 10, 29), (33, 43, 0), (34, 0, 11), (34, 12, 41),
    (34, 13, 162), (34, 44, 0), (35, 1, 126), (35, 5, 152),
    (35, 11, 172), (35, 45, 0), (36, 0, 73), (36, 2, 154),
    (36, 7, 129), (36, 46, 0), (37, 10, 167), (37, 13, 38),
    (37, 47, 0), (38, 1, 112), (38, 5, 7), (38, 11, 19),
    (38, 48, 0), (39, 0, 109), (39, 7, 6), (39, 12, 105),
    (39, 49, 0), (40, 2, 160), (40, 10, 156), (40, 13, 82),
    (40, 50, 0), (41, 1, 132), (41, 5, 6), (41, 11, 8),
    (41, 51, 0),
];

// ---------------------------------------------------------------------
// Validator 1: dimensions
// ---------------------------------------------------------------------

fn validate_dimensions() -> Result<(), String> {
    let mut rows_seen = [false; BASE_ROWS];
    let mut cols_seen = [false; BASE_COLS];

    for &(r, c, s) in ENTRIES {
        if r >= BASE_ROWS {
            return Err(format!("row {r} out of range (BASE_ROWS={BASE_ROWS})"));
        }
        if c >= BASE_COLS {
            return Err(format!("col {c} out of range (BASE_COLS={BASE_COLS})"));
        }
        if s >= ZC {
            return Err(format!("shift {s} out of range at ({r},{c}) (ZC={ZC})"));
        }
        rows_seen[r] = true;
        cols_seen[c] = true;
    }

    if let Some(r) = rows_seen.iter().position(|&seen| !seen) {
        return Err(format!("row {r} has no entries"));
    }
    if let Some(c) = cols_seen.iter().position(|&seen| !seen) {
        return Err(format!("col {c} has no entries"));
    }
    if ENTRIES.is_empty() {
        return Err("no entries at all".to_string());
    }

    Ok(())
}

// ---------------------------------------------------------------------
// Validator 2: zero 4-cycles at Zc=176
// ---------------------------------------------------------------------

/// A length-4 cycle in a lifted QC-LDPC graph exists iff two base rows share
/// two base columns `(c1, c2)` such that the shift difference
/// `(s1 - s2) mod Zc` is the *same* in both rows (the two rows' lifted edges
/// then form a 4-cycle through those two column-blocks). Standard check, see
/// e.g. Fossorier, "Quasi-Cyclic Low-Density Parity-Check Codes From
/// Circulant Permutation Matrices" (2004).
fn validate_no_4_cycles() -> Result<(), String> {
    use std::collections::HashMap;

    // Group entries by base row.
    let mut by_row: Vec<Vec<(usize, usize)>> = vec![Vec::new(); BASE_ROWS];
    for &(r, c, s) in ENTRIES {
        by_row[r].push((c, s));
    }

    // seen[(c1, c2, delta)] -> row index that first produced it.
    let mut seen: HashMap<(usize, usize, i64), usize> = HashMap::new();

    for (r, edges) in by_row.iter().enumerate() {
        for i in 0..edges.len() {
            for j in 0..edges.len() {
                if i == j {
                    continue;
                }
                let (c1, s1) = edges[i];
                let (c2, s2) = edges[j];
                if c1 == c2 {
                    continue; // base graph never repeats a column within a row here
                }
                let delta = ((s1 as i64 - s2 as i64) % ZC as i64 + ZC as i64) % ZC as i64;
                let key = (c1, c2, delta);
                if let Some(&prev_row) = seen.get(&key) {
                    if prev_row != r {
                        return Err(format!(
                            "4-cycle: base cols ({c1},{c2}) share shift-delta {delta} \
                             in rows {prev_row} and {r} (Zc={ZC})"
                        ));
                    }
                } else {
                    seen.insert(key, r);
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------
// Validator 3: min column weight >= 2, scoped to core columns
// ---------------------------------------------------------------------

fn validate_min_col_weight_core() -> Result<(), String> {
    let mut col_weight = vec![0usize; BASE_COLS];
    for &(_, c, _) in ENTRIES {
        col_weight[c] += 1;
    }

    let core_cols = KB + CORE_PARITY_COLS; // 14
    for (c, &w) in col_weight.iter().enumerate().take(core_cols) {
        if w < 2 {
            return Err(format!(
                "core column {c} has weight {w} (< 2) -- likely a transcription defect"
            ));
        }
    }

    // Sanity-print (not a failure condition): confirm extension columns are
    // exactly the documented degree-1 "single-row-sum" columns, so the
    // scoping decision above is visibly justified, not silently assumed.
    let ext_all_degree_1 = col_weight[core_cols..].iter().all(|&w| w == 1);
    println!(
        "    (info) extension columns [{core_cols}..{BASE_COLS}) all have degree exactly 1: {ext_all_degree_1}"
    );
    println!(
        "    (info) core column weights (0..{core_cols}): {:?}",
        &col_weight[..core_cols]
    );

    Ok(())
}

// ---------------------------------------------------------------------
// GF(2) dense bit-matrix (small: used only for the 4*Zc x 4*Zc core inverse)
// ---------------------------------------------------------------------

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

    /// Gauss-Jordan inversion over GF(2). Returns `None` if singular.
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
            // Find a pivot row with a 1 in this column, at or below `col`.
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

// ---------------------------------------------------------------------
// Generic (table-driven) two-step systematic encoder, for the round-trip
// validator. Mirrors the algorithm the production coppa-protocol encoder
// implements (see crates/coppa-protocol/src/fec/ldpc/encoder.rs); written
// independently here (not shared code) so the two implementations serve as
// a cross-check of each other, not a single shared point of failure.
// ---------------------------------------------------------------------

/// Lifted block matvec: for edges `(row, col, shift)` filtered to
/// `row in [row_lo, row_lo+row_n)` and `col in [col_lo, col_lo+col_n)`,
/// XOR-accumulate `x[(col-col_lo)*Z + (i+shift)%Z]` into
/// `out[(row-row_lo)*Z + i]` for every `i` in `0..Z`.
fn block_matvec(
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

/// Build the dense lifted `hm_b` (core parity) matrix: base rows
/// `0..CORE_PARITY_COLS`, base cols `KB..KB+CORE_PARITY_COLS`, each lifted to
/// a Zc x Zc circulant-shift block.
fn build_core_matrix(entries: &[(usize, usize, usize)], z: usize) -> Gf2Matrix {
    let n = CORE_PARITY_COLS * z;
    let mut m = Gf2Matrix::zeros(n, n);
    for &(r, c, s) in entries {
        if r >= CORE_PARITY_COLS || !(KB..KB + CORE_PARITY_COLS).contains(&c) {
            continue;
        }
        let br = r;
        let bc = c - KB;
        for i in 0..z {
            m.set(br * z + i, bc * z + (i + s) % z, true);
        }
    }
    m
}

/// Full two-step systematic encode: `info` has length `KB*Z` (1760);
/// returns the "mother codeword" of length `(BASE_COLS - KB - PUNCTURED_INFO_COLS)*Z`
/// ... actually returns `(BASE_COLS - PUNCTURED_INFO_COLS)*Z` = 8800 bits:
/// `[info[PUNCTURED_INFO_COLS*Z..], parity]`, matching the production
/// `NrLdpc::encode` contract exactly (see rate_match.rs docs).
fn encode_mother(entries: &[(usize, usize, usize)], z: usize, info: &[u8]) -> Vec<u8> {
    assert_eq!(info.len(), KB * z);

    // rhs_a = hm_a * info  (hm_a: rows 0..4, cols 0..KB)
    let rhs_a = block_matvec(entries, z, 0, CORE_PARITY_COLS, 0, KB, info);

    let core = build_core_matrix(entries, z);
    let core_inv = core
        .invert()
        .expect("BG2 core submatrix must be invertible over GF(2)");
    let p_a = core_inv.mul_vec(&rhs_a);

    // p_b[row] = (hm_c1 * info)[row] xor (hm_c2 * p_a)[row], row-by-row single sums.
    let ext_rows = BASE_ROWS - CORE_PARITY_COLS;
    let c1 = block_matvec(entries, z, CORE_PARITY_COLS, ext_rows, 0, KB, info);
    let c2 = block_matvec(
        entries,
        z,
        CORE_PARITY_COLS,
        ext_rows,
        KB,
        CORE_PARITY_COLS,
        &p_a,
    );
    let p_b: Vec<u8> = c1.iter().zip(c2.iter()).map(|(&a, &b)| a ^ b).collect();

    let mut mother = Vec::with_capacity((BASE_COLS - PUNCTURED_INFO_COLS) * z);
    mother.extend_from_slice(&info[PUNCTURED_INFO_COLS * z..]);
    mother.extend_from_slice(&p_a);
    mother.extend_from_slice(&p_b);
    mother
}

/// Check every one of the `BASE_ROWS*Z` lifted parity equations against the
/// full codeword (systematic info, all `KB*Z` bits, reconstructed from
/// `info` directly -- not from `mother`, since `mother` excludes the
/// punctured columns).
fn check_codeword(entries: &[(usize, usize, usize)], z: usize, full_codeword: &[u8]) -> bool {
    for row in 0..BASE_ROWS {
        for i in 0..z {
            let mut syn = 0u8;
            for &(r, c, s) in entries {
                if r != row {
                    continue;
                }
                syn ^= full_codeword[c * z + (i + s) % z];
            }
            if syn != 0 {
                return false;
            }
        }
    }
    true
}

// ---------------------------------------------------------------------
// Validator 4: encode/check round-trip
// ---------------------------------------------------------------------

fn validate_roundtrip() -> Result<(), String> {
    // A handful of deterministic pseudo-random info patterns (no external
    // rand dependency needed for a generator tool -- a simple LCG suffices).
    let mut seed: u64 = 0x9E3779B97F4A7C15;
    let mut next_bit = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed & 1) as u8
    };

    for trial in 0..8 {
        let info: Vec<u8> = (0..KB * ZC).map(|_| next_bit()).collect();
        let mother = encode_mother(ENTRIES, ZC, &info);
        if mother.len() != (BASE_COLS - PUNCTURED_INFO_COLS) * ZC {
            return Err(format!(
                "trial {trial}: mother length {} != {}",
                mother.len(),
                (BASE_COLS - PUNCTURED_INFO_COLS) * ZC
            ));
        }

        // Reassemble the FULL codeword (all KB*Z info bits + all parity) to
        // run the complete H*c=0 check across all 42 base rows.
        let mut full = Vec::with_capacity(BASE_COLS * ZC);
        full.extend_from_slice(&info[..PUNCTURED_INFO_COLS * ZC]);
        full.extend_from_slice(&mother);

        if !check_codeword(ENTRIES, ZC, &full) {
            return Err(format!("trial {trial}: H*c^T != 0"));
        }
    }

    // All-zero info must produce the all-zero codeword (sanity).
    let info0 = vec![0u8; KB * ZC];
    let mother0 = encode_mother(ENTRIES, ZC, &info0);
    if mother0.iter().any(|&b| b != 0) {
        return Err("all-zero info did not produce an all-zero mother codeword".to_string());
    }

    Ok(())
}

// ---------------------------------------------------------------------
// Codegen
// ---------------------------------------------------------------------

fn codegen() -> String {
    let mut out = String::new();
    writeln!(
        out,
        "//! Auto-generated by `tools/gen_nr_bg2` -- DO NOT EDIT BY HAND."
    )
    .unwrap();
    writeln!(out, "//! Regenerate with `cargo run -p gen_nr_bg2`.").unwrap();
    writeln!(out, "//!").unwrap();
    writeln!(
        out,
        "//! 3GPP TS 38.212 §5.3.2 Table 5.3.2-3 (BG2), lifting-size family"
    )
    .unwrap();
    writeln!(
        out,
        "//! i_LS = 5 (Zc in {{11, 22, 44, 88, 176, 352}}), fixed here at Zc = 176."
    )
    .unwrap();
    writeln!(out, "//!").unwrap();
    writeln!(
        out,
        "//! Full transcription provenance, cross-validation methodology, and the"
    )
    .unwrap();
    writeln!(
        out,
        "//! `i_LS` correction (the family containing 176 is index 5, not 7) are"
    )
    .unwrap();
    writeln!(
        out,
        "//! documented in `tools/gen_nr_bg2/src/main.rs`'s module docs -- read those"
    )
    .unwrap();
    writeln!(
        out,
        "//! before editing this file's *source of truth* (the generator), not this"
    )
    .unwrap();
    writeln!(out, "//! generated output.").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "/// Lifting factor. Fixed: one lifting size for the whole speed-level ladder."
    )
    .unwrap();
    writeln!(out, "pub const ZC: usize = {ZC};").unwrap();
    writeln!(
        out,
        "/// Number of base-graph check rows (= number of parity columns)."
    )
    .unwrap();
    writeln!(out, "pub const BASE_ROWS: usize = {BASE_ROWS};").unwrap();
    writeln!(
        out,
        "/// Number of base-graph columns (10 info + 42 parity)."
    )
    .unwrap();
    writeln!(out, "pub const BASE_COLS: usize = {BASE_COLS};").unwrap();
    writeln!(
        out,
        "/// Number of information columns in the base graph (\"K_b\")."
    )
    .unwrap();
    writeln!(out, "pub const KB: usize = {KB};").unwrap();
    writeln!(
        out,
        "/// Number of \"core\" dual-diagonal parity columns (fixed NR LDPC constant)."
    )
    .unwrap();
    writeln!(
        out,
        "pub const CORE_PARITY_COLS: usize = {CORE_PARITY_COLS};"
    )
    .unwrap();
    writeln!(
        out,
        "/// Number of Zc-blocks of systematic (info) columns punctured at the wire."
    )
    .unwrap();
    writeln!(
        out,
        "pub const PUNCTURED_INFO_COLS: usize = {PUNCTURED_INFO_COLS};"
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "/// Transcribed (base_row, base_col, shift) entries. {} non-zero entries.",
        ENTRIES.len()
    )
    .unwrap();
    writeln!(out, "#[rustfmt::skip]").unwrap();
    writeln!(out, "pub const ENTRIES: &[(usize, usize, usize)] = &[").unwrap();
    for chunk in ENTRIES.chunks(4) {
        let line: String = chunk
            .iter()
            .map(|(r, c, s)| format!("({r}, {c}, {s})"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "    {line},").unwrap();
    }
    writeln!(out, "];").unwrap();
    out
}

fn main() {
    println!("gen_nr_bg2: validating transcribed BG2 (i_LS=5, Zc=176) base graph...");

    let mut ok = true;

    print!("  [1/4] dimensions... ");
    match validate_dimensions() {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL: {e}");
            ok = false;
        }
    }

    print!("  [2/4] zero 4-cycles at Zc={ZC}... ");
    match validate_no_4_cycles() {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL: {e}");
            ok = false;
        }
    }

    print!(
        "  [3/4] min column weight >= 2 (core columns 0..{}): ",
        KB + CORE_PARITY_COLS
    );
    match validate_min_col_weight_core() {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL: {e}");
            ok = false;
        }
    }

    print!("  [4/4] encode/check round-trip H*c^T=0... ");
    match validate_roundtrip() {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL: {e}");
            ok = false;
        }
    }

    if !ok {
        eprintln!("gen_nr_bg2: one or more validators FAILED -- refusing to write nr_bg2.rs");
        std::process::exit(1);
    }

    let out_path = "../../crates/coppa-protocol/src/fec/ldpc/nr_bg2.rs";
    let generated = codegen();
    std::fs::write(out_path, generated).expect("failed to write nr_bg2.rs");
    println!("gen_nr_bg2: all validators passed; wrote {out_path}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_pass() {
        validate_dimensions().unwrap();
    }

    #[test]
    fn no_4_cycles() {
        validate_no_4_cycles().unwrap();
    }

    #[test]
    fn min_col_weight_core() {
        validate_min_col_weight_core().unwrap();
    }

    #[test]
    fn roundtrip() {
        validate_roundtrip().unwrap();
    }

    #[test]
    fn extension_columns_are_degree_one_by_design() {
        // Documents (as an executable assertion, not just a comment) exactly
        // why validator 3 is scoped to the core: the full-graph extension
        // zone is *supposed* to be all degree-1.
        let mut col_weight = vec![0usize; BASE_COLS];
        for &(_, c, _) in ENTRIES {
            col_weight[c] += 1;
        }
        let core_cols = KB + CORE_PARITY_COLS;
        for (c, &w) in col_weight.iter().enumerate().skip(core_cols) {
            assert_eq!(w, 1, "extension column {c} expected degree 1, got {w}");
        }
    }

    #[test]
    fn entry_count_matches_cross_validated_source_count() {
        // Both independent sources (Sionna, srsRAN) agreed on exactly 197
        // non-zero entries for this family -- pin that count so a future
        // accidental edit is caught immediately.
        assert_eq!(ENTRIES.len(), 197);
    }
}
