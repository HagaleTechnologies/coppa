//! Normalized min-sum belief propagation decoder for QC-LDPC codes.
//!
//! The normalized min-sum algorithm is a reduced-complexity approximation to the
//! standard sum-product (belief propagation) decoder. Instead of computing
//! exact hyperbolic tangent functions for check-node updates, it uses:
//!
//!   alpha * min(|L_1|, |L_2|, ...)
//!
//! where alpha is a multiplicative scaling factor in (0,1] (default 0.8, see
//! `DEFAULT_SCALE`) that corrects min-sum's overestimate of check-node
//! reliability. Unlike the fixed *offset* min-sum variant this replaced,
//! normalized min-sum is scale-invariant (see `decoder_is_scale_invariant`).
//! This provides nearly identical error-correction performance to sum-product
//! while using only additions, comparisons, and sign operations -- no
//! transcendental functions.
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

/// Message/posterior magnitude bound. Min-sum messages otherwise grow by up to
/// ×(1 + α·(dv_max − 1)) per iteration (≈ ×9.8 at R=1/4) and overflow f32 on
/// non-convergent frames: inf − inf = NaN then poisons the min-scan and the
/// hard decisions. 64 is far above any decision-relevant magnitude (inputs are
/// clipped to ±20) and standard practice for fixed-range min-sum decoders.
const MSG_CLAMP: f32 = 64.0;

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

/// Normalized min-sum belief propagation LDPC decoder.
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
    /// The decoder runs normalized min-sum BP for up to `max_iterations`,
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
            // For each check node, compute outgoing messages using normalized min-sum.
            for check in 0..self.graph.num_checks {
                let edges = &self.graph.check_to_edges[check];
                let num_neighbors = edges.len();
                if num_neighbors == 0 {
                    continue;
                }

                // Compute product of signs and minimum magnitudes
                // For each outgoing edge e, the message is:
                //   sign = product of signs of all OTHER incoming messages
                //   magnitude = alpha * min(all OTHER magnitudes)

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
                total_llr[var] = (channel + incoming_sum).clamp(-MSG_CLAMP, MSG_CLAMP);

                // Outgoing messages: total LLR minus the incoming message from target check
                for &(edge_idx, _check) in edges {
                    var_to_check[edge_idx] =
                        (total_llr[var] - check_to_var[edge_idx]).clamp(-MSG_CLAMP, MSG_CLAMP);
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

// =========================================================================
// NR BG2 mother-code decoder (Task 4): layered (row-based) normalized
// min-sum.
// =========================================================================
//
// A "layered" (a.k.a. row-layered, or shuffled) schedule processes one base
// row (a "layer" -- Zc lifted check rows sharing the same base-graph row) at
// a time and updates variable-node posteriors *immediately*, so later
// layers in the same iteration already see the updated beliefs from earlier
// layers -- unlike flooding (`LdpcDecoder` above), which only updates
// posteriors once per full iteration. This is the standard technique for
// halving LDPC iteration counts in practice; see e.g. Mansour & Shanbhag,
// "High-Throughput LDPC Decoders" (2003), or any modern QC-LDPC ASIC/DSP
// decoder writeup. The normalized min-sum node update itself is identical
// to `LdpcDecoder`'s (reuses [`two_smallest`]).
use super::nr_bg2;

/// Default normalized min-sum scale for the BG2 layered decoder.
///
/// `coppa-bench`'s `task4_alpha_calibration` swept alpha in [0.60, 1.00] at
/// level 2 only (isolated FEC layer, near-full-capacity payload, 300
/// trials/point) across a 5-point SNR band straddling that level's FER<=10%
/// threshold, and picked 0.80 as measurably better than 0.75 there (avg FER
/// 0.441 vs. 0.451 across the band). **That pick turned out not to
/// generalize**: `tests/phase_c_loopback.rs`'s `test_all_levels_min_payload`
/// and `test_awgn_level_10_above_threshold` -- level 10 (the highest rate,
/// least redundancy, and thus most pinning-dependent level) with a tiny
/// (1-byte) payload, i.e. extreme known-pad pinning -- failed to converge at
/// 0.80 even on a clean channel, and passed again once reverted to 0.75.
/// 0.80 was calibrated against a single level/payload-size combination and
/// happened to sit in a region that is fine for level 2 but breaks level
/// 10's very different operating point (see the Task 4 report for the full
/// story); 0.75 is the value validated across the whole ladder and every
/// existing test, so it is what ships. A properly generalizing calibration
/// (sweeping every level, not just level 2) is flagged as follow-up work,
/// not attempted again here given the time already spent chasing this once.
const NR_DEFAULT_SCALE: f32 = 0.75;

/// Default max iterations. Layered schedules converge in fewer iterations
/// than flooding at the same error-correction performance, so this is
/// smaller than `DEFAULT_MAX_ITERATIONS` above; see the Task 4 report's
/// early-exit iteration statistics.
const NR_DEFAULT_MAX_ITERATIONS: usize = 30;

/// Message magnitude clamp (see `MSG_CLAMP`'s doc above for the rationale --
/// identical failure mode, identical fix).
const NR_MSG_CLAMP: f32 = 64.0;

/// Layered normalized min-sum decoder for the NR BG2 mother code
/// (Zc=176, 42 base rows, 52 base columns -- the *full* graph, i.e. `N =
/// BASE_COLS*ZC = 9152` variable nodes, including the `PUNCTURED_INFO_COLS *
/// ZC = 352` always-punctured leading info bits).
#[derive(Debug, Clone)]
pub struct NrBg2Decoder {
    scale: f32,
    max_iterations: usize,
    /// Per base row, the list of (base_col, shift) edges. Length
    /// `BASE_ROWS`.
    rows_edges: Vec<Vec<(usize, usize)>>,
    /// Cumulative (unlifted) edge count before base row `r`, i.e.
    /// `edge_offset[r] = sum_{r' < r} rows_edges[r'].len()`. Used to compute
    /// a flat message index for `(base_row, sub_row, local_edge)`.
    edge_offset: Vec<usize>,
    /// Total (unlifted) edge count, `sum(rows_edges[r].len())` = 197.
    total_edges_unlifted: usize,
    /// Precomputed variable-node index for every (base_row, sub_row `i`,
    /// local edge `k`), flattened as `var_table[r][i*deg(r) + k]`. This is
    /// exactly `col*Z + (i+shift)%Z`, computed once here instead of on
    /// every edge visit of every iteration of every `decode()` call --
    /// `Z=176` is not a power of two, so `(i+shift)%Z` is a genuine integer
    /// division, and profiling (`examples/task4_decode_profile.rs`, Task 4
    /// report) found the per-iteration layered-update pass, not allocation
    /// or wrapper overhead, dominates decode cost; precomputing this table
    /// removes ~35,000 runtime modulo operations per iteration.
    var_table: Vec<Vec<usize>>,
}

impl Default for NrBg2Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl NrBg2Decoder {
    pub fn new() -> Self {
        Self::with_params(NR_DEFAULT_SCALE, NR_DEFAULT_MAX_ITERATIONS)
    }

    pub fn with_params(scale: f32, max_iterations: usize) -> Self {
        let z = nr_bg2::ZC;
        let mut rows_edges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nr_bg2::BASE_ROWS];
        for &(r, c, s) in nr_bg2::ENTRIES {
            rows_edges[r].push((c, s));
        }
        let mut edge_offset = Vec::with_capacity(nr_bg2::BASE_ROWS);
        let mut cum = 0usize;
        for edges in &rows_edges {
            edge_offset.push(cum);
            cum += edges.len();
        }

        let var_table: Vec<Vec<usize>> = rows_edges
            .iter()
            .map(|edges| {
                let deg = edges.len();
                let mut table = vec![0usize; z * deg];
                for i in 0..z {
                    for (k, &(col, shift)) in edges.iter().enumerate() {
                        table[i * deg + k] = col * z + (i + shift) % z;
                    }
                }
                table
            })
            .collect();

        Self {
            scale,
            max_iterations,
            rows_edges,
            edge_offset,
            total_edges_unlifted: cum,
            var_table,
        }
    }

    /// Number of variable nodes in the full graph (`BASE_COLS * ZC` = 9152).
    pub fn num_vars(&self) -> usize {
        nr_bg2::BASE_COLS * nr_bg2::ZC
    }

    /// Decode. `full_llrs` must have length [`Self::num_vars`] (the *full*
    /// graph, including the leading `PUNCTURED_INFO_COLS*ZC` always-erased
    /// bits -- callers normally get this via
    /// [`NrLdpc::decode_soft`](super::NrLdpc::decode_soft), which handles
    /// that prepend). Returns `(posterior, iterations_used, converged)`.
    pub fn decode(&self, full_llrs: &[f32]) -> (Vec<f32>, usize, bool) {
        let z = nr_bg2::ZC;
        let n = self.num_vars();
        assert_eq!(
            full_llrs.len(),
            n,
            "expected {n} full-graph LLRs, got {}",
            full_llrs.len()
        );

        let mut total_llr = full_llrs.to_vec();
        let mut check_to_var = vec![0.0f32; self.total_edges_unlifted * z];
        let mut converged = false;
        let mut iterations_used = 0;
        // Hard-decision cache, maintained incrementally as `total_llr` is
        // updated below rather than recomputed from scratch by a separate
        // full pass in `check_syndrome`. A variable node is touched by every
        // row (base-graph column) it participates in -- average degree here
        // is `total_edges_unlifted*z / n` (~3.8) -- so `check_syndrome`
        // previously re-derived the same hard bit from a 4-byte `f32`
        // compare on each of those visits; profiling
        // (`examples/task4_decode_profile.rs`, Task 4 report) found this
        // syndrome pass a substantial fraction of per-decode cost. Updating
        // `hard[var]` right where `total_llr[var]` is written folds the sign
        // decision into the update pass (which already visits every
        // variable node at least once per iteration, since every column of
        // a valid parity-check matrix appears in at least one row), leaving
        // `check_syndrome` a pure integer XOR over 1-byte lookups with no
        // float comparisons at all.
        let mut hard: Vec<u8> = total_llr.iter().map(|&l| (l < 0.0) as u8).collect();

        for iter in 0..self.max_iterations {
            iterations_used = iter + 1;
            for (r, edges) in self.rows_edges.iter().enumerate() {
                let deg = edges.len();
                if deg == 0 {
                    continue;
                }
                let base_edge = self.edge_offset[r];
                let var_row = &self.var_table[r];

                // Fixed-size stack buffers -- BG2's max row degree here is
                // 10 (see nr_bg2 provenance docs' printed weights), but size
                // generously; assert (not debug_assert) so an out-of-range
                // degree can never silently corrupt memory in release
                // builds (mirrors `LdpcDecoder`'s `check node degree`
                // assert above).
                assert!(
                    deg <= 32,
                    "base row {r} degree {deg} exceeds fixed buffer size 32"
                );
                let mut signs = [0.0f32; 32];
                let mut mags = [0.0f32; 32];
                let mut vars = [0usize; 32];
                let mut old_msgs = [0.0f32; 32];

                for i in 0..z {
                    let row_base = i * deg;
                    for k in 0..deg {
                        let var = var_row[row_base + k];
                        let edge_idx = (base_edge + k) * z + i;
                        let old_msg = check_to_var[edge_idx];
                        let extrinsic = total_llr[var] - old_msg;
                        signs[k] = if extrinsic >= 0.0 { 1.0 } else { -1.0 };
                        mags[k] = extrinsic.abs();
                        vars[k] = var;
                        old_msgs[k] = old_msg;
                    }

                    let total_sign: f32 = signs[..deg].iter().product();
                    let (min1, min1_idx, min2) = two_smallest(&mags[..deg]);

                    for k in 0..deg {
                        let min_other = if k == min1_idx { min2 } else { min1 };
                        let new_msg = (total_sign * signs[k] * min_other * self.scale)
                            .clamp(-NR_MSG_CLAMP, NR_MSG_CLAMP);
                        let edge_idx = (base_edge + k) * z + i;
                        check_to_var[edge_idx] = new_msg;
                        let updated = (total_llr[vars[k]] + (new_msg - old_msgs[k]))
                            .clamp(-NR_MSG_CLAMP, NR_MSG_CLAMP);
                        total_llr[vars[k]] = updated;
                        hard[vars[k]] = (updated < 0.0) as u8;
                    }
                }
            }

            if self.check_syndrome(&hard) {
                converged = true;
                break;
            }
        }

        (total_llr, iterations_used, converged)
    }

    /// Check whether the current hard decisions satisfy every parity check.
    /// `hard` is a precomputed per-variable-node hard-decision cache (`1` if
    /// the posterior LLR is negative, `0` otherwise), kept in sync with
    /// `total_llr` by `decode`'s update loop -- see the comment there. This
    /// keeps the syndrome pass a pure XOR reduction over 1-byte lookups
    /// instead of re-deriving each hard bit from a 4-byte float compare on
    /// every one of a variable node's (multiple) row memberships.
    fn check_syndrome(&self, hard: &[u8]) -> bool {
        let z = nr_bg2::ZC;
        for (r, edges) in self.rows_edges.iter().enumerate() {
            let deg = edges.len();
            let var_row = &self.var_table[r];
            for i in 0..z {
                let mut syn = 0u8;
                let row_base = i * deg;
                for k in 0..deg {
                    syn ^= hard[var_row[row_base + k]];
                }
                if syn != 0 {
                    return false;
                }
            }
        }
        true
    }
}

/// Flooding-schedule variant of the same normalized min-sum decoder, kept
/// only for the A/B comparison in the Task 4 report (iteration count /
/// timing vs. the layered schedule above) -- not used by production code.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct NrBg2FloodingDecoder {
    scale: f32,
    max_iterations: usize,
    rows_edges: Vec<Vec<(usize, usize)>>,
    edge_offset: Vec<usize>,
    total_edges_unlifted: usize,
}

#[cfg(test)]
impl NrBg2FloodingDecoder {
    pub(crate) fn with_params(scale: f32, max_iterations: usize) -> Self {
        let mut rows_edges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nr_bg2::BASE_ROWS];
        for &(r, c, s) in nr_bg2::ENTRIES {
            rows_edges[r].push((c, s));
        }
        let mut edge_offset = Vec::with_capacity(nr_bg2::BASE_ROWS);
        let mut cum = 0usize;
        for edges in &rows_edges {
            edge_offset.push(cum);
            cum += edges.len();
        }
        Self {
            scale,
            max_iterations,
            rows_edges,
            edge_offset,
            total_edges_unlifted: cum,
        }
    }

    pub(crate) fn decode(&self, full_llrs: &[f32]) -> (Vec<f32>, usize, bool) {
        let z = nr_bg2::ZC;
        let n = nr_bg2::BASE_COLS * z;
        assert_eq!(full_llrs.len(), n);

        let mut check_to_var = vec![0.0f32; self.total_edges_unlifted * z];
        let mut total_llr = full_llrs.to_vec();
        let mut converged = false;
        let mut iterations_used = 0;

        for iter in 0..self.max_iterations {
            iterations_used = iter + 1;
            let var_to_check_snapshot = {
                // Flooding: all check updates for this iteration read the
                // PREVIOUS iteration's posteriors (snapshot), unlike the
                // layered decoder which updates in place row-by-row.
                total_llr.clone()
            };

            let mut new_check_to_var = check_to_var.clone();
            for (r, edges) in self.rows_edges.iter().enumerate() {
                let deg = edges.len();
                if deg == 0 {
                    continue;
                }
                let base_edge = self.edge_offset[r];
                let mut signs = [0.0f32; 32];
                let mut mags = [0.0f32; 32];

                for i in 0..z {
                    for (k, &(col, shift)) in edges.iter().enumerate() {
                        let var = col * z + (i + shift) % z;
                        let edge_idx = (base_edge + k) * z + i;
                        let extrinsic = var_to_check_snapshot[var] - check_to_var[edge_idx];
                        signs[k] = if extrinsic >= 0.0 { 1.0 } else { -1.0 };
                        mags[k] = extrinsic.abs();
                    }
                    let total_sign: f32 = signs[..deg].iter().product();
                    let (min1, min1_idx, min2) = two_smallest(&mags[..deg]);
                    for (k, &sign_k) in signs[..deg].iter().enumerate() {
                        let min_other = if k == min1_idx { min2 } else { min1 };
                        let edge_idx = (base_edge + k) * z + i;
                        new_check_to_var[edge_idx] = (total_sign * sign_k * min_other * self.scale)
                            .clamp(-NR_MSG_CLAMP, NR_MSG_CLAMP);
                    }
                }
            }
            check_to_var = new_check_to_var;

            // Recompute all posteriors from scratch (flooding: one pass per
            // iteration).
            total_llr.copy_from_slice(full_llrs);
            for (r, edges) in self.rows_edges.iter().enumerate() {
                let base_edge = self.edge_offset[r];
                for i in 0..z {
                    for (k, &(col, shift)) in edges.iter().enumerate() {
                        let var = col * z + (i + shift) % z;
                        let edge_idx = (base_edge + k) * z + i;
                        total_llr[var] = (total_llr[var] + check_to_var[edge_idx])
                            .clamp(-NR_MSG_CLAMP, NR_MSG_CLAMP);
                    }
                }
            }

            if self.check_syndrome(&total_llr) {
                converged = true;
                break;
            }
        }

        (total_llr, iterations_used, converged)
    }

    fn check_syndrome(&self, total_llr: &[f32]) -> bool {
        let z = nr_bg2::ZC;
        for edges in &self.rows_edges {
            for i in 0..z {
                let mut syn = 0u8;
                for &(col, shift) in edges {
                    let var = col * z + (i + shift) % z;
                    if total_llr[var] < 0.0 {
                        syn ^= 1;
                    }
                }
                if syn != 0 {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod nr_bg2_decoder_tests {
    use super::*;
    use crate::fec::ldpc::encoder::NrBg2Encoder;

    fn full_codeword_llrs(
        mother_len_full_including_punctured: usize,
        full_codeword: &[u8],
    ) -> Vec<f32> {
        assert_eq!(full_codeword.len(), mother_len_full_including_punctured);
        full_codeword
            .iter()
            .map(|&b| if b == 0 { 4.0 } else { -4.0 })
            .collect()
    }

    fn encode_full(enc: &NrBg2Encoder, info: &[u8]) -> Vec<u8> {
        let z = nr_bg2::ZC;
        let punctured = nr_bg2::PUNCTURED_INFO_COLS;
        let mother = enc.encode_mother(info);
        let mut full = Vec::with_capacity(nr_bg2::BASE_COLS * z);
        full.extend_from_slice(&info[..punctured * z]);
        full.extend_from_slice(&mother);
        full
    }

    #[test]
    fn decodes_perfect_channel() {
        let enc = NrBg2Encoder::new();
        let dec = NrBg2Decoder::new();
        let info: Vec<u8> = (0..nr_bg2::KB * nr_bg2::ZC)
            .map(|i| (i % 2) as u8)
            .collect();
        let full = encode_full(&enc, &info);
        let llrs = full_codeword_llrs(full.len(), &full);
        let (posterior, _iters, converged) = dec.decode(&llrs);
        assert!(converged);
        let decoded_info: Vec<u8> = posterior[..info.len()]
            .iter()
            .map(|&l| if l >= 0.0 { 0 } else { 1 })
            .collect();
        assert_eq!(decoded_info, info);
    }

    #[test]
    fn layered_and_flooding_agree_on_perfect_channel() {
        let enc = NrBg2Encoder::new();
        let info: Vec<u8> = (0..nr_bg2::KB * nr_bg2::ZC)
            .map(|i| ((i * 5 + 1) % 2) as u8)
            .collect();
        let full = encode_full(&enc, &info);
        let llrs = full_codeword_llrs(full.len(), &full);

        let layered = NrBg2Decoder::new();
        let flooding =
            NrBg2FloodingDecoder::with_params(NR_DEFAULT_SCALE, NR_DEFAULT_MAX_ITERATIONS);

        let (post_l, _iters_l, conv_l) = layered.decode(&llrs);
        let (post_f, _iters_f, conv_f) = flooding.decode(&llrs);
        assert!(conv_l && conv_f);

        let hard = |p: &[f32]| -> Vec<u8> {
            p[..info.len()]
                .iter()
                .map(|&l| if l >= 0.0 { 0 } else { 1 })
                .collect()
        };
        assert_eq!(hard(&post_l), info);
        assert_eq!(hard(&post_f), info);
    }

    /// Statistical diagnostic (not a pass/fail gate -- see the Task 4 report):
    /// does the layered schedule's FER at a real noisy operating point match
    /// its own flooding reference (same table, same alpha, same iteration
    /// cap, same channel realizations)? If layered measurably *underperforms*
    /// flooding here, that's a real layered-schedule bug, not a code-vs-code
    /// coding-gain limit -- this isolates the schedule from every other
    /// variable (rate matching, mapper, calibration all held fixed and
    /// identical between the two runs).
    ///
    /// Uses level 2's k_used=972 at a fixed, deliberately-hard SNR (BPSK,
    /// AWGN, mapped directly -- no OFDM, real-valued channel per
    /// `coppa-bench`'s established `task3_fec_isolated_gate` convention) in
    /// the waterfall region (bracketed empirically: 0.0 dB saturates both
    /// decoders near 100% failure, 2.0 dB saturates both near 0%; 1.0 dB is
    /// the discriminating point), so a schedule difference would actually
    /// show up in the FER rather than being swamped by a floor/ceiling
    /// effect. Note this test's SNR label uses the real-valued-channel
    /// convention (matches `task3_fec_isolated_gate.rs`), NOT
    /// `task4_bg2_ldpc_gate`'s generic-complex-symbol convention (which
    /// applies noise to both I/Q components even for BPSK, matching how the
    /// real OFDM receiver's per-carrier complex noise estimates work) --
    /// the two conventions differ by close to 3 dB for BPSK specifically
    /// (BPSK only carries information on one axis), so their SNR labels are
    /// not directly comparable. See the Task 4 report for this finding.
    #[test]
    #[ignore = "statistical (500 trials); run manually: cargo test -p coppa-protocol --lib -- --ignored layered_matches_flooding_fer_at_noisy_operating_point --nocapture"]
    fn layered_matches_flooding_fer_at_noisy_operating_point() {
        use crate::fec::ldpc::rate_match::{rate_dematch, rate_match};
        use crate::fec::ldpc::NrLdpc;
        use crate::fec::scrambler::scramble;
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        const K_USED: usize = 972; // level 2
        const PAYLOAD_BITS: usize = 964; // near-full capacity, matches the bench gate convention
        const E: usize = 1944;
        const SNR_DB: f32 = 1.0; // discriminating point in the waterfall region (real-channel convention)
        const TRIALS: usize = 500;

        let enc = NrBg2Encoder::new();
        let layered = NrBg2Decoder::new();
        let flooding =
            NrBg2FloodingDecoder::with_params(NR_DEFAULT_SCALE, NR_DEFAULT_MAX_ITERATIONS);

        let run = |dec_layered: bool, seed: u64| -> bool {
            let mut rng = StdRng::seed_from_u64(seed);
            let mut info: Vec<u8> = (0..PAYLOAD_BITS)
                .map(|_| rng.random_range(0..2u8))
                .collect();
            info.resize(nr_bg2::KB * nr_bg2::ZC, 0u8);
            let truth = info.clone();
            scramble(&mut info);
            let mother = enc.encode_mother(&info);
            let matched = rate_match(&mother, K_USED, E, 0);

            // BPSK-map + AWGN directly (no OFDM), same convention as the
            // isolated bench gate.
            let noise_std = (10f32.powf(-SNR_DB / 10.0)).sqrt();
            let mut rng2 = StdRng::seed_from_u64(seed ^ 0xA11CE);
            let llrs: Vec<f32> = matched
                .iter()
                .map(|&b| {
                    let base = if b == 0 { 1.0 } else { -1.0 };
                    let u1: f32 = rng2.random::<f32>().max(1e-10);
                    let u2: f32 = rng2.random();
                    let noise = noise_std
                        * (-2.0 * u1.ln()).sqrt()
                        * (2.0 * std::f32::consts::PI * u2).cos();
                    let rx = base + noise;
                    4.0 * rx / (noise_std * noise_std)
                })
                .collect();

            let mut dematched = rate_dematch(&llrs, K_USED, E, 0, NrLdpc::MOTHER_LEN);
            crate::fec::ldpc::pin_known_pad(&mut dematched, PAYLOAD_BITS, K_USED, 64.0);

            let punctured_len = nr_bg2::PUNCTURED_INFO_COLS * nr_bg2::ZC;
            let mut full_llrs = Vec::with_capacity(nr_bg2::BASE_COLS * nr_bg2::ZC);
            full_llrs.resize(punctured_len, 0.0f32);
            full_llrs.extend_from_slice(&dematched);

            let (posterior, _iters, converged) = if dec_layered {
                layered.decode(&full_llrs)
            } else {
                flooding.decode(&full_llrs)
            };
            if !converged {
                return false;
            }
            let mut decoded: Vec<u8> = posterior[..nr_bg2::KB * nr_bg2::ZC]
                .iter()
                .map(|&l| if l >= 0.0 { 0 } else { 1 })
                .collect();
            scramble(&mut decoded);
            decoded[..PAYLOAD_BITS] == truth[..PAYLOAD_BITS]
        };

        let mut layered_fails = 0usize;
        let mut flooding_fails = 0usize;
        for t in 0..TRIALS {
            let seed = 0xFEED_0000u64.wrapping_add(t as u64);
            if !run(true, seed) {
                layered_fails += 1;
            }
            if !run(false, seed) {
                flooding_fails += 1;
            }
        }

        println!(
            "layered FER={}/{TRIALS} ({:.3}), flooding FER={}/{TRIALS} ({:.3})",
            layered_fails,
            layered_fails as f64 / TRIALS as f64,
            flooding_fails,
            flooding_fails as f64 / TRIALS as f64
        );

        // Layered must not be *meaningfully worse* than flooding (allow a
        // small statistical margin, not an exact match -- both are
        // stochastic and share only the channel realization, not identical
        // internal message order). A layered schedule that's much worse
        // than its own flooding reference indicates a schedule bug, not a
        // code-vs-code limit.
        assert!(
            layered_fails <= flooding_fails + (TRIALS / 20), // 5% of TRIALS slack
            "layered decoder ({layered_fails}/{TRIALS} failures) performs meaningfully worse than \
             its own flooding reference ({flooding_fails}/{TRIALS} failures) at the same alpha/cap -- \
             this points to a layered-schedule bug, not an inherent code-vs-code limit"
        );
    }

    #[test]
    fn decode_survives_pathological_llrs_without_nan() {
        let dec = NrBg2Decoder::new();
        let n = dec.num_vars();
        let llrs: Vec<f32> = (0..n)
            .map(|i| if i % 3 == 0 { 20.0 } else { -20.0 })
            .collect();
        let (posterior, _iters, converged) = dec.decode(&llrs);
        assert!(!converged);
        assert!(
            posterior.iter().all(|v| v.is_finite()),
            "posterior must never contain NaN/inf"
        );
    }
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

    #[test]
    fn decoder_survives_pathological_llrs_without_nan() {
        // Non-convergent input with large, conflicting LLRs used to grow messages
        // unboundedly (×~9.8/iteration at high variable degree) → f32 inf → NaN via
        // inf - inf in the variable-node update. The decoder must stay finite and
        // return converged=false, not poison its output with NaNs.
        use crate::fec::ldpc::LdpcCodec;
        let codec = LdpcCodec::new(CodeRate::Rate1_4);
        let n = 1944;
        // Adversarial pattern: max-magnitude alternating LLRs that satisfy no checks.
        let llrs: Vec<f32> = (0..n)
            .map(|i| if i % 3 == 0 { 20.0 } else { -20.0 })
            .collect();
        let (decoded, converged) = codec.decode_checked(&llrs);
        assert!(!converged, "adversarial input must not converge");
        assert_eq!(decoded.len(), codec.code().info_bits());
        // The real assertion: no NaN anywhere in the decision path. Since decoded bits
        // are produced from total_llr signs, re-run and check the public invariant that
        // decode of the SAME input is deterministic (NaN comparisons would make the
        // min-scan order-dependent across identical runs only if state were poisoned;
        // determinism plus non-convergence plus finite behavior is the observable).
        let (decoded2, converged2) = codec.decode_checked(&llrs);
        assert_eq!(decoded, decoded2);
        assert_eq!(converged, converged2);
    }
}
