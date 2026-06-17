//! Block and frequency interleavers for Coppa.
//!
//! Interleaving spreads consecutive bit errors across the codeword so that
//! the FEC decoder sees scattered errors rather than a burst.
//!
//! **Block interleaver**: writes data row-by-row into a matrix, reads column-by-column.
//! A burst of `cols` consecutive errors becomes `cols` single errors spread across
//! `cols` different rows (codewords).
//!
//! **Frequency interleaver**: pseudorandom permutation of OFDM subcarriers per symbol,
//! ensuring that adjacent coded bits map to widely-spaced subcarriers. This protects
//! against frequency-selective fading.

use anyhow::{anyhow, Result};

/// Block interleaver: write rows, read columns.
///
/// For a matrix of `rows x cols`:
/// - Writing: data fills row 0 left-to-right, then row 1, etc.
/// - Reading: data is read column 0 top-to-bottom, then column 1, etc.
///
/// The interleaver depth (number of rows) determines burst error tolerance:
/// a burst of up to `cols` consecutive errors is spread across `cols` rows.
#[derive(Debug, Clone)]
pub struct BlockInterleaver {
    rows: usize,
    cols: usize,
}

impl BlockInterleaver {
    /// Create a new block interleaver with the given dimensions.
    pub fn new(rows: usize, cols: usize) -> Result<Self> {
        if rows == 0 || cols == 0 {
            return Err(anyhow!(
                "Interleaver dimensions must be positive: {}x{}",
                rows,
                cols
            ));
        }
        Ok(Self { rows, cols })
    }

    /// Total capacity of the interleaver matrix.
    pub fn capacity(&self) -> usize {
        self.rows * self.cols
    }

    /// Number of rows (interleaving depth).
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Number of columns.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Interleave: write rows, read columns.
    ///
    /// Input length must equal `rows * cols`. Use `interleave_padded` if the input
    /// may be shorter.
    pub fn interleave(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() != self.capacity() {
            return Err(anyhow!(
                "Data length {} != interleaver capacity {}",
                data.len(),
                self.capacity()
            ));
        }

        let mut output = Vec::with_capacity(data.len());

        // Read column by column
        for col in 0..self.cols {
            for row in 0..self.rows {
                output.push(data[row * self.cols + col]);
            }
        }

        Ok(output)
    }

    /// Deinterleave: write columns, read rows (inverse of interleave).
    pub fn deinterleave(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() != self.capacity() {
            return Err(anyhow!(
                "Data length {} != interleaver capacity {}",
                data.len(),
                self.capacity()
            ));
        }

        let mut output = Vec::with_capacity(data.len());

        // Read row by row
        for row in 0..self.rows {
            for col in 0..self.cols {
                output.push(data[col * self.rows + row]);
            }
        }

        Ok(output)
    }

    /// Interleave with zero-padding if data is shorter than capacity.
    /// Returns the interleaved data (always `rows * cols` elements).
    pub fn interleave_padded(&self, data: &[u8]) -> Vec<u8> {
        let cap = self.capacity();
        let mut padded = Vec::with_capacity(cap);
        padded.extend_from_slice(data);
        padded.resize(cap, 0);
        // Safe to unwrap: we just ensured the length matches
        self.interleave(&padded).unwrap()
    }

    /// Deinterleave and truncate to original length.
    pub fn deinterleave_truncated(&self, data: &[u8], original_len: usize) -> Result<Vec<u8>> {
        let deinterleaved = self.deinterleave(data)?;
        Ok(deinterleaved[..original_len.min(deinterleaved.len())].to_vec())
    }

    /// Interleave soft symbols (f32 values, e.g., LLRs).
    pub fn interleave_soft(&self, data: &[f32]) -> Result<Vec<f32>> {
        if data.len() != self.capacity() {
            return Err(anyhow!(
                "Data length {} != interleaver capacity {}",
                data.len(),
                self.capacity()
            ));
        }

        let mut output = Vec::with_capacity(data.len());

        for col in 0..self.cols {
            for row in 0..self.rows {
                output.push(data[row * self.cols + col]);
            }
        }

        Ok(output)
    }

    /// Deinterleave soft symbols (f32 values).
    pub fn deinterleave_soft(&self, data: &[f32]) -> Result<Vec<f32>> {
        if data.len() != self.capacity() {
            return Err(anyhow!(
                "Data length {} != interleaver capacity {}",
                data.len(),
                self.capacity()
            ));
        }

        let mut output = Vec::with_capacity(data.len());

        for row in 0..self.rows {
            for col in 0..self.cols {
                output.push(data[col * self.rows + row]);
            }
        }

        Ok(output)
    }
}

/// Frequency interleaver: pseudorandom permutation of subcarrier indices.
///
/// Generates a deterministic permutation for a given number of subcarriers
/// and seed, ensuring that adjacent coded bits map to widely-spaced
/// subcarriers in the OFDM symbol.
#[derive(Debug, Clone)]
pub struct FrequencyInterleaver {
    /// Number of subcarriers.
    num_carriers: usize,
    /// Forward permutation table: output[perm[i]] = input[i].
    perm: Vec<usize>,
    /// Inverse permutation table: output[inv_perm[i]] = input[i].
    inv_perm: Vec<usize>,
}

impl FrequencyInterleaver {
    /// Create a frequency interleaver for `num_carriers` subcarriers.
    ///
    /// The `seed` parameter allows different permutations per OFDM symbol,
    /// providing frequency diversity across time.
    pub fn new(num_carriers: usize, seed: u32) -> Result<Self> {
        if num_carriers == 0 {
            return Err(anyhow!("Number of carriers must be positive"));
        }

        let perm = Self::generate_permutation(num_carriers, seed);
        let mut inv_perm = vec![0usize; num_carriers];
        for (i, &p) in perm.iter().enumerate() {
            inv_perm[p] = i;
        }

        Ok(Self {
            num_carriers,
            perm,
            inv_perm,
        })
    }

    /// Number of subcarriers.
    pub fn num_carriers(&self) -> usize {
        self.num_carriers
    }

    /// Get the forward permutation table.
    pub fn permutation(&self) -> &[usize] {
        &self.perm
    }

    /// Interleave: apply the permutation.
    pub fn interleave<T: Copy + Default>(&self, data: &[T]) -> Result<Vec<T>> {
        if data.len() != self.num_carriers {
            return Err(anyhow!(
                "Data length {} != num_carriers {}",
                data.len(),
                self.num_carriers
            ));
        }

        let mut output = vec![T::default(); self.num_carriers];
        for (i, &p) in self.perm.iter().enumerate() {
            output[p] = data[i];
        }

        Ok(output)
    }

    /// Deinterleave: apply the inverse permutation.
    pub fn deinterleave<T: Copy + Default>(&self, data: &[T]) -> Result<Vec<T>> {
        if data.len() != self.num_carriers {
            return Err(anyhow!(
                "Data length {} != num_carriers {}",
                data.len(),
                self.num_carriers
            ));
        }

        let mut output = vec![T::default(); self.num_carriers];
        for (i, &p) in self.inv_perm.iter().enumerate() {
            output[p] = data[i];
        }

        Ok(output)
    }

    /// Generate a pseudorandom permutation using a simple LCG-based
    /// Fisher-Yates shuffle.
    ///
    /// This is deterministic for a given seed, lightweight, and produces
    /// a uniform permutation.
    fn generate_permutation(n: usize, seed: u32) -> Vec<usize> {
        let mut perm: Vec<usize> = (0..n).collect();

        // LCG: x_{n+1} = (a * x_n + c) mod 2^64
        // Multiplier and increment are Knuth's MMIX LCG constants
        // (a = 6364136223846793005, c = 1442695040888963407).
        let mut state = seed as u64;

        for i in (1..n).rev() {
            // Advance LCG
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let j = (state >> 33) as usize % (i + 1);
            perm.swap(i, j);
        }

        perm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Block interleaver tests ─────────────────────────────────────

    #[test]
    fn test_block_interleave_roundtrip() {
        let il = BlockInterleaver::new(4, 6).unwrap();
        let data: Vec<u8> = (0..24).collect();

        let interleaved = il.interleave(&data).unwrap();
        assert_ne!(interleaved, data); // should be permuted

        let deinterleaved = il.deinterleave(&interleaved).unwrap();
        assert_eq!(deinterleaved, data);
    }

    #[test]
    fn test_block_interleave_known_pattern() {
        // 3x4 matrix:
        // Write rows:   [0  1  2  3]
        //               [4  5  6  7]
        //               [8  9 10 11]
        //
        // Read columns: [0 4 8] [1 5 9] [2 6 10] [3 7 11]
        // Result:        0 4 8  1 5 9  2 6 10  3 7 11
        let il = BlockInterleaver::new(3, 4).unwrap();
        let data: Vec<u8> = (0..12).collect();
        let interleaved = il.interleave(&data).unwrap();
        assert_eq!(interleaved, vec![0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11]);
    }

    #[test]
    fn test_block_interleave_square() {
        let il = BlockInterleaver::new(4, 4).unwrap();
        let data: Vec<u8> = (0..16).collect();

        let interleaved = il.interleave(&data).unwrap();
        let deinterleaved = il.deinterleave(&interleaved).unwrap();
        assert_eq!(deinterleaved, data);
    }

    #[test]
    fn test_block_interleave_1x_n() {
        // 1 row: interleaving is identity
        let il = BlockInterleaver::new(1, 8).unwrap();
        let data: Vec<u8> = (0..8).collect();
        let interleaved = il.interleave(&data).unwrap();
        assert_eq!(interleaved, data);
    }

    #[test]
    fn test_block_interleave_n_x_1() {
        // 1 column: interleaving is identity
        let il = BlockInterleaver::new(8, 1).unwrap();
        let data: Vec<u8> = (0..8).collect();
        let interleaved = il.interleave(&data).unwrap();
        assert_eq!(interleaved, data);
    }

    #[test]
    fn test_block_interleave_wrong_length() {
        let il = BlockInterleaver::new(3, 4).unwrap();
        assert!(il.interleave(&[0; 10]).is_err());
        assert!(il.deinterleave(&[0; 10]).is_err());
    }

    #[test]
    fn test_block_interleave_zero_dims() {
        assert!(BlockInterleaver::new(0, 4).is_err());
        assert!(BlockInterleaver::new(4, 0).is_err());
    }

    #[test]
    fn test_block_interleave_padded() {
        let il = BlockInterleaver::new(3, 4).unwrap();
        let data: Vec<u8> = vec![1, 2, 3, 4, 5]; // 5 < 12
        let interleaved = il.interleave_padded(&data);
        assert_eq!(interleaved.len(), 12);

        let deinterleaved = il.deinterleave_truncated(&interleaved, 5).unwrap();
        assert_eq!(deinterleaved, data);
    }

    #[test]
    fn test_block_interleave_burst_spreading() {
        // A burst of `cols` consecutive errors in the interleaved domain
        // should become single errors spread across different rows.
        let rows = 8;
        let cols = 6;
        let il = BlockInterleaver::new(rows, cols).unwrap();

        let data: Vec<u8> = vec![0; rows * cols];
        let mut interleaved = il.interleave(&data).unwrap();

        // Simulate a burst error of length `rows` starting at position 0
        // (first column's worth of data)
        for b in interleaved.iter_mut().take(rows) {
            *b = 1;
        }

        let deinterleaved = il.deinterleave(&interleaved).unwrap();

        // The errors should be spread: one per row, each in column 0
        let mut error_count_per_row = vec![0u32; rows];
        for (i, &val) in deinterleaved.iter().enumerate() {
            if val != 0 {
                error_count_per_row[i / cols] += 1;
            }
        }

        // Each row should have at most 1 error
        for count in &error_count_per_row {
            assert!(
                *count <= 1,
                "Burst not properly spread: {:?}",
                error_count_per_row
            );
        }
    }

    #[test]
    fn test_block_interleave_soft_roundtrip() {
        let il = BlockInterleaver::new(4, 8).unwrap();
        let data: Vec<f32> = (0..32).map(|i| i as f32 * 0.1).collect();

        let interleaved = il.interleave_soft(&data).unwrap();
        let deinterleaved = il.deinterleave_soft(&interleaved).unwrap();

        for (a, b) in data.iter().zip(deinterleaved.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_block_interleave_soft_wrong_length() {
        let il = BlockInterleaver::new(3, 4).unwrap();
        assert!(il.interleave_soft(&[0.0; 10]).is_err());
        assert!(il.deinterleave_soft(&[0.0; 10]).is_err());
    }

    #[test]
    fn test_block_interleave_large() {
        let il = BlockInterleaver::new(32, 64).unwrap();
        let data: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();

        let interleaved = il.interleave(&data).unwrap();
        let deinterleaved = il.deinterleave(&interleaved).unwrap();
        assert_eq!(deinterleaved, data);
    }

    #[test]
    fn test_block_capacity() {
        let il = BlockInterleaver::new(5, 7).unwrap();
        assert_eq!(il.capacity(), 35);
        assert_eq!(il.rows(), 5);
        assert_eq!(il.cols(), 7);
    }

    // ── Frequency interleaver tests ─────────────────────────────────

    #[test]
    fn test_freq_interleave_roundtrip() {
        let fi = FrequencyInterleaver::new(64, 42).unwrap();
        let data: Vec<u8> = (0..64).map(|i| (i % 256) as u8).collect();

        let interleaved = fi.interleave(&data).unwrap();
        assert_ne!(interleaved, data);

        let deinterleaved = fi.deinterleave(&interleaved).unwrap();
        assert_eq!(deinterleaved, data);
    }

    #[test]
    fn test_freq_interleave_is_permutation() {
        let fi = FrequencyInterleaver::new(32, 0).unwrap();
        let perm = fi.permutation();

        // Every index 0..32 should appear exactly once
        let mut sorted = perm.to_vec();
        sorted.sort();
        let expected: Vec<usize> = (0..32).collect();
        assert_eq!(sorted, expected);
    }

    #[test]
    fn test_freq_interleave_different_seeds() {
        let fi1 = FrequencyInterleaver::new(64, 1).unwrap();
        let fi2 = FrequencyInterleaver::new(64, 2).unwrap();

        // Different seeds should produce different permutations
        assert_ne!(fi1.permutation(), fi2.permutation());
    }

    #[test]
    fn test_freq_interleave_deterministic() {
        let fi1 = FrequencyInterleaver::new(64, 42).unwrap();
        let fi2 = FrequencyInterleaver::new(64, 42).unwrap();

        // Same seed should produce same permutation
        assert_eq!(fi1.permutation(), fi2.permutation());
    }

    #[test]
    fn test_freq_interleave_small() {
        let fi = FrequencyInterleaver::new(4, 0).unwrap();
        let data: Vec<u8> = vec![10, 20, 30, 40];

        let interleaved = fi.interleave(&data).unwrap();
        let deinterleaved = fi.deinterleave(&interleaved).unwrap();
        assert_eq!(deinterleaved, data);
    }

    #[test]
    fn test_freq_interleave_single() {
        let fi = FrequencyInterleaver::new(1, 0).unwrap();
        let data: Vec<u8> = vec![42];

        let interleaved = fi.interleave(&data).unwrap();
        assert_eq!(interleaved, data);
    }

    #[test]
    fn test_freq_interleave_wrong_length() {
        let fi = FrequencyInterleaver::new(8, 0).unwrap();
        assert!(fi.interleave(&[0u8; 4]).is_err());
        assert!(fi.deinterleave(&[0u8; 4]).is_err());
    }

    #[test]
    fn test_freq_interleave_zero_carriers() {
        assert!(FrequencyInterleaver::new(0, 0).is_err());
    }

    #[test]
    fn test_freq_interleave_f32() {
        let fi = FrequencyInterleaver::new(16, 7).unwrap();
        let data: Vec<f32> = (0..16).map(|i| i as f32 * 1.5).collect();

        let interleaved = fi.interleave(&data).unwrap();
        let deinterleaved = fi.deinterleave(&interleaved).unwrap();

        for (a, b) in data.iter().zip(deinterleaved.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_freq_interleave_spreading() {
        // Adjacent inputs should map to non-adjacent outputs
        let fi = FrequencyInterleaver::new(64, 123).unwrap();
        let perm = fi.permutation();

        // Count how many adjacent input pairs map to adjacent output positions
        let mut adjacent_count = 0;
        for i in 0..(perm.len() - 1) {
            let diff = (perm[i] as isize - perm[i + 1] as isize).unsigned_abs();
            if diff <= 1 {
                adjacent_count += 1;
            }
        }

        // A good interleaver should have very few adjacent mappings
        // For 64 carriers, random chance gives ~2/64 ~ 3%
        assert!(
            adjacent_count < 10,
            "Too many adjacent mappings: {}/63",
            adjacent_count
        );
    }

    #[test]
    fn test_freq_interleave_large() {
        let fi = FrequencyInterleaver::new(1024, 999).unwrap();
        let data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();

        let interleaved = fi.interleave(&data).unwrap();
        let deinterleaved = fi.deinterleave(&interleaved).unwrap();
        assert_eq!(deinterleaved, data);
    }

    // ── Combined block + frequency interleaver test ─────────────────

    #[test]
    fn test_combined_block_freq_roundtrip() {
        let block = BlockInterleaver::new(8, 64).unwrap();
        let freq = FrequencyInterleaver::new(64, 42).unwrap();

        let data: Vec<u8> = (0..512).map(|i| (i % 256) as u8).collect();

        // Interleave: block first, then frequency per row
        let block_interleaved = block.interleave(&data).unwrap();

        // Apply frequency interleaving to each "column group" (each set of 8 values)
        // In practice, freq interleaving would be per OFDM symbol.
        // Here we just test the combined pipeline.
        let freq_interleaved: Vec<u8> = block_interleaved
            .chunks(64)
            .flat_map(|chunk| freq.interleave(chunk).unwrap())
            .collect();

        // Deinterleave: frequency first, then block
        let freq_deinterleaved: Vec<u8> = freq_interleaved
            .chunks(64)
            .flat_map(|chunk| freq.deinterleave(chunk).unwrap())
            .collect();

        let result = block.deinterleave(&freq_deinterleaved).unwrap();
        assert_eq!(result, data);
    }
}
