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
                if output.len() < n {
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
                if i < n {
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
    fn test_interleave_roundtrip() {
        let interleaver = BlockInterleaver::new(1944, 44);
        let bits: Vec<u8> = (0..1944).map(|i| (i % 2) as u8).collect();
        let interleaved = interleaver.interleave(&bits);
        let as_llr: Vec<f32> = interleaved
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        let deinterleaved = interleaver.deinterleave(&as_llr);
        let recovered: Vec<u8> = deinterleaved
            .iter()
            .map(|&l| if l > 0.0 { 0 } else { 1 })
            .collect();
        assert_eq!(bits, recovered);
    }

    #[test]
    fn test_interleave_permutes_bits() {
        let interleaver = BlockInterleaver::new(12, 4);
        let bits: Vec<u8> = (0..12).collect();
        let interleaved = interleaver.interleave(&bits);
        assert_eq!(interleaved, vec![0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11]);
    }

    #[test]
    fn test_interleave_with_padding() {
        let interleaver = BlockInterleaver::new(10, 4);
        let bits: Vec<u8> = (0..10).collect();
        let interleaved = interleaver.interleave(&bits);
        assert_eq!(interleaved.len(), 10);
    }

    #[test]
    fn test_deinterleave_llr_roundtrip() {
        let interleaver = BlockInterleaver::new(1944, 44);
        let llrs: Vec<f32> = (0..1944).map(|i| i as f32 * 0.1).collect();
        let bits: Vec<u8> = llrs.iter().map(|&l| if l >= 0.0 { 0 } else { 1 }).collect();
        let interleaved = interleaver.interleave(&bits);
        let interleaved_llrs: Vec<f32> = interleaved
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        let deinterleaved = interleaver.deinterleave(&interleaved_llrs);
        for (orig, recov) in llrs.iter().zip(deinterleaved.iter()) {
            assert_eq!(orig.signum(), recov.signum(), "Sign mismatch");
        }
    }
}
