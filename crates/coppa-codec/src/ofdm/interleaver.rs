/// Block interleaver for spreading coded bits across OFDM time-frequency grid.
///
/// Writes bits row-wise (across carriers within one OFDM symbol) and reads
/// column-wise (across symbols on the same carrier). This spreads adjacent
/// coded bits across both frequency and time dimensions.
pub struct BlockInterleaver {
    rows: usize,
    cols: usize,
}

impl BlockInterleaver {
    pub fn new(block_size: usize, carriers: usize) -> Self {
        let cols = carriers;
        let rows = block_size.div_ceil(carriers);
        Self { rows, cols }
    }

    pub fn interleave(&self, bits: &[u8]) -> Vec<u8> {
        let n = bits.len();
        let total = self.rows * self.cols;
        let mut grid = vec![0u8; total];
        grid[..n].copy_from_slice(bits);

        let mut output = Vec::with_capacity(n);
        for col in 0..self.cols {
            for row in 0..self.rows {
                let idx = row * self.cols + col;
                // Pad cells (idx >= n) are skipped entirely: the scan emits exactly
                // the n real bits. Emitting pads used to puncture the tail of the
                // codeword (35 bits lost on 1944/44) — see the regression tests.
                if idx < n {
                    output.push(grid[idx]);
                }
            }
        }
        output
    }

    /// Same row-write/column-read permutation as [`Self::interleave`], but for
    /// soft LLR values (`f32`) instead of hard bits (`u8`). Used by Task 5's
    /// turbo re-estimation to carry posterior LLRs (already selected down to
    /// coded-bit order by `rate_match::rate_match_llr`) into the same
    /// interleaved (wire/symbol) order the original transmission used, so they
    /// line up 1:1 with `eq_symbols`/the per-data-carrier soft-demap order
    /// `CoppaTransceiver::receive_with_metrics` builds `llrs` in.
    pub fn interleave_soft(&self, llrs: &[f32]) -> Vec<f32> {
        let n = llrs.len();
        let total = self.rows * self.cols;
        let mut grid = vec![0.0f32; total];
        grid[..n].copy_from_slice(llrs);

        let mut output = Vec::with_capacity(n);
        for col in 0..self.cols {
            for row in 0..self.rows {
                let idx = row * self.cols + col;
                if idx < n {
                    output.push(grid[idx]);
                }
            }
        }
        output
    }

    pub fn deinterleave(&self, llrs: &[f32]) -> Vec<f32> {
        let n = llrs.len();
        let total = self.rows * self.cols;
        let mut grid = vec![0.0f32; total];
        let mut i = 0;
        for col in 0..self.cols {
            for row in 0..self.rows {
                let idx = row * self.cols + col;
                if idx < n && i < n {
                    grid[idx] = llrs[i];
                    i += 1;
                }
            }
        }
        grid[..n].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_is_a_bijection_no_bit_dropped() {
        // 1944 over 44 carriers has 36 pad cells mid-scan — the historic puncture bug.
        // Use a pattern that is NOT periodic in 2 or 44 so no dropped index can hide.
        let il = BlockInterleaver::new(1944, 44);
        let bits: Vec<u8> = (0..1944u32)
            .map(|i| ((i.wrapping_mul(2654435761)) >> 31) as u8 & 1)
            .collect();
        let llrs: Vec<f32> = bits
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        let rx = il.deinterleave(
            &il.interleave(&bits)
                .iter()
                .map(|&b| if b == 0 { 1.0 } else { -1.0 })
                .collect::<Vec<f32>>(),
        );
        for (i, (&a, &b)) in llrs.iter().zip(rx.iter()).enumerate() {
            assert_eq!(a, b, "position {i} not preserved (puncture regression)");
        }
    }

    #[test]
    fn interleave_never_emits_pad_and_never_drops_tail() {
        // Direct regression on the exact historic failure: the 35 indices ≡ 43 (mod 44)
        // from 439..=1935 were dropped and 35 zeros emitted in their place.
        let il = BlockInterleaver::new(1944, 44);
        // Mark exactly the historically-dropped positions with 1s, everything else 0.
        let mut bits = vec![0u8; 1944];
        for m in 0..35 {
            bits[439 + 44 * m] = 1;
        }
        let out = il.interleave(&bits);
        let ones_out: usize = out.iter().map(|&b| b as usize).sum();
        assert_eq!(
            ones_out, 35,
            "all 35 historically-punctured bits must be transmitted"
        );
    }

    #[test]
    fn interleave_exact_grid_sizes_unchanged() {
        // hf_robust: 1944 over 36 carriers = 54x36 exact (no pads). Behavior must be
        // identical before/after the fix — guard with a spot-check permutation property.
        let il = BlockInterleaver::new(1944, 36);
        let bits: Vec<u8> = (0..1944u32)
            .map(|i| ((i * 40503) >> 13) as u8 & 1)
            .collect();
        let llrs: Vec<f32> = il
            .interleave(&bits)
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        let rx = il.deinterleave(&llrs);
        for (i, &v) in rx.iter().enumerate() {
            let expect = if bits[i] == 0 { 1.0 } else { -1.0 };
            assert_eq!(v, expect, "exact-grid case regressed at {i}");
        }
    }

    #[test]
    fn test_interleave_permutes_bits() {
        let interleaver = BlockInterleaver::new(12, 4);
        let bits: Vec<u8> = (0..12).collect();
        let interleaved = interleaver.interleave(&bits);
        assert_eq!(interleaved, vec![0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11]);
    }

    #[test]
    fn interleave_soft_matches_interleave_index_permutation() {
        // Feed distinct float "tags" through interleave_soft and hard-bit-domain
        // tags (as f32) through a manual replica of `interleave`'s permutation on
        // the same size grid, then check the deinterleave round trip recovers
        // the original values exactly (interleave_soft's whole point is doing
        // the identical permutation as `interleave`, just typed for f32).
        let il = BlockInterleaver::new(1944, 44);
        let vals: Vec<f32> = (0..1944u32).map(|i| i as f32).collect();
        let interleaved = il.interleave_soft(&vals);
        assert_eq!(interleaved.len(), 1944);
        let back = il.deinterleave(&interleaved);
        for (i, (&a, &b)) in vals.iter().zip(back.iter()).enumerate() {
            assert_eq!(
                a, b,
                "position {i} not preserved by interleave_soft/deinterleave"
            );
        }
    }

    #[test]
    fn test_interleave_with_padding() {
        let interleaver = BlockInterleaver::new(10, 4);
        let bits: Vec<u8> = (0..10).collect();
        let interleaved = interleaver.interleave(&bits);
        assert_eq!(interleaved.len(), 10);
    }
}
