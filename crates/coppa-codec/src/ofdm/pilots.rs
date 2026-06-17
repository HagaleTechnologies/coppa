//! Pilot subcarrier management for OFDM channel estimation and phase tracking.
use num_complex::Complex32;

/// Generates pilot subcarrier indices for a given OFDM profile.
///
/// Pilots are evenly distributed among the active subcarriers to enable
/// channel estimation via interpolation.
pub struct PilotPattern {
    /// Indices of pilot subcarriers within the active carrier set.
    pub pilot_indices: Vec<usize>,
    /// Indices of data subcarriers within the active carrier set.
    pub data_indices: Vec<usize>,
    /// Known pilot values (BPSK, typically all +1 or PN sequence).
    pub pilot_values: Vec<Complex32>,
}

impl PilotPattern {
    /// Create a pilot pattern with evenly spaced pilots.
    pub fn new(num_active: usize, num_pilots: usize) -> Self {
        let mut pilot_indices = Vec::with_capacity(num_pilots);
        let mut data_indices = Vec::with_capacity(num_active - num_pilots);
        let mut pilot_values = Vec::with_capacity(num_pilots);

        if num_pilots == 0 || num_active == 0 {
            return Self {
                pilot_indices,
                data_indices: (0..num_active).collect(),
                pilot_values,
            };
        }

        // Place pilots to include edges for better channel estimation coverage.
        // First and last active carriers get pilots, rest are evenly spaced.
        if num_pilots >= 2 {
            pilot_indices.push(0); // first edge
            pilot_values.push(Complex32::new(1.0, 0.0));

            let interior = num_pilots - 2;
            if interior > 0 {
                let spacing = (num_active - 1) / (interior + 1);
                for i in 0..interior {
                    let idx = spacing * (i + 1);
                    if idx < num_active && !pilot_indices.contains(&idx) {
                        pilot_indices.push(idx);
                        pilot_values.push(Complex32::new(1.0, 0.0));
                    }
                }
            }

            pilot_indices.push(num_active - 1); // last edge
            pilot_values.push(Complex32::new(1.0, 0.0));
        } else {
            // Single pilot or none — place in center
            for i in 0..num_pilots {
                let idx = num_active / (num_pilots + 1) * (i + 1);
                if idx < num_active {
                    pilot_indices.push(idx);
                    pilot_values.push(Complex32::new(1.0, 0.0));
                }
            }
        }

        pilot_indices.sort();
        pilot_indices.dedup();

        for i in 0..num_active {
            if !pilot_indices.contains(&i) {
                data_indices.push(i);
            }
        }

        Self {
            pilot_indices,
            data_indices,
            pilot_values,
        }
    }

    /// Insert data symbols and pilot values into a full subcarrier vector.
    pub fn insert(&self, data_symbols: &[Complex32]) -> Vec<Complex32> {
        let total = self.pilot_indices.len() + self.data_indices.len();
        let mut output = vec![Complex32::new(0.0, 0.0); total];

        for (i, &idx) in self.pilot_indices.iter().enumerate() {
            if idx < total {
                output[idx] = self.pilot_values[i];
            }
        }

        for (i, &idx) in self.data_indices.iter().enumerate() {
            if idx < total && i < data_symbols.len() {
                output[idx] = data_symbols[i];
            }
        }

        output
    }

    /// Extract data symbols from a full subcarrier vector (removing pilots).
    pub fn extract_data(&self, subcarriers: &[Complex32]) -> Vec<Complex32> {
        self.data_indices
            .iter()
            .filter_map(|&idx| subcarriers.get(idx).copied())
            .collect()
    }

    /// Extract pilot values from a full subcarrier vector.
    pub fn extract_pilots(&self, subcarriers: &[Complex32]) -> Vec<(usize, Complex32)> {
        self.pilot_indices
            .iter()
            .filter_map(|&idx| subcarriers.get(idx).map(|&v| (idx, v)))
            .collect()
    }
}

/// Pilot pattern that alternates pilot positions between even and odd OFDM symbols.
///
/// Even symbols place pilots at evenly-spaced positions; odd symbols offset by half
/// the pilot spacing so that together they provide denser channel estimation coverage.
#[derive(Debug, Clone)]
pub struct CoppaPilotPattern {
    total_carriers: usize,
    num_pilots: usize,
    even_indices: Vec<usize>,
    odd_indices: Vec<usize>,
}

impl CoppaPilotPattern {
    /// Create an alternating pilot pattern.
    ///
    /// Even symbols place pilots at evenly-spaced positions starting from 0.
    /// Odd symbols offset each position by half the spacing.
    pub fn new(total_carriers: usize, num_pilots: usize) -> Self {
        let (even_indices, odd_indices) = if num_pilots == 0 || total_carriers == 0 {
            (vec![], vec![])
        } else {
            let spacing = total_carriers / num_pilots;
            let half_spacing = spacing / 2;

            let even: Vec<usize> = (0..num_pilots)
                .map(|i| (i * spacing).min(total_carriers - 1))
                .collect();

            let odd: Vec<usize> = (0..num_pilots)
                .map(|i| ((i * spacing) + half_spacing).min(total_carriers - 1))
                .collect();

            (even, odd)
        };

        Self {
            total_carriers,
            num_pilots,
            even_indices,
            odd_indices,
        }
    }

    /// Return pilot indices for the given symbol number (even/odd alternation).
    pub fn pilot_indices(&self, symbol_num: usize) -> &[usize] {
        if symbol_num % 2 == 0 {
            &self.even_indices
        } else {
            &self.odd_indices
        }
    }

    /// Return all non-pilot carrier indices for the given symbol number.
    pub fn data_indices(&self, symbol_num: usize) -> Vec<usize> {
        let pilots = self.pilot_indices(symbol_num);
        (0..self.total_carriers)
            .filter(|i| !pilots.contains(i))
            .collect()
    }

    /// Number of data carriers per symbol (total_carriers - num_pilots).
    pub fn num_data(&self) -> usize {
        self.total_carriers.saturating_sub(self.num_pilots)
    }

    /// Insert +1.0 pilots into a full-length carrier vector, filling data positions
    /// from `data` in order.
    ///
    /// Returns a vector of length `total_carriers`.
    pub fn insert_pilots(&self, data: &[Complex32], symbol_num: usize) -> Vec<Complex32> {
        let pilots = self.pilot_indices(symbol_num);
        let data_idx = self.data_indices(symbol_num);

        let mut output = vec![Complex32::new(0.0, 0.0); self.total_carriers];

        for &idx in pilots {
            if idx < self.total_carriers {
                output[idx] = Complex32::new(1.0, 0.0);
            }
        }

        for (slot, &idx) in data_idx.iter().enumerate() {
            if idx < self.total_carriers {
                if let Some(&val) = data.get(slot) {
                    output[idx] = val;
                }
            }
        }

        output
    }

    /// Extract data carriers (non-pilot positions) from a full carrier vector.
    pub fn extract_data(&self, carriers: &[Complex32], symbol_num: usize) -> Vec<Complex32> {
        self.data_indices(symbol_num)
            .iter()
            .filter_map(|&idx| carriers.get(idx).copied())
            .collect()
    }

    /// Extract pilot carrier values along with their indices.
    pub fn extract_pilots(
        &self,
        carriers: &[Complex32],
        symbol_num: usize,
    ) -> Vec<(usize, Complex32)> {
        self.pilot_indices(symbol_num)
            .iter()
            .filter_map(|&idx| carriers.get(idx).map(|&v| (idx, v)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pilot_pattern_creation() {
        let pattern = PilotPattern::new(76, 9);
        assert_eq!(pattern.pilot_indices.len(), 9);
        assert_eq!(pattern.data_indices.len(), 67);
        assert_eq!(pattern.pilot_values.len(), 9);
    }

    #[test]
    fn test_pilot_insert_extract_roundtrip() {
        let pattern = PilotPattern::new(16, 4);
        let data: Vec<Complex32> = (0..pattern.data_indices.len())
            .map(|i| Complex32::new(i as f32, 0.0))
            .collect();

        let full = pattern.insert(&data);
        assert_eq!(full.len(), 16);

        let extracted = pattern.extract_data(&full);
        assert_eq!(extracted.len(), data.len());
        for (a, b) in data.iter().zip(extracted.iter()) {
            assert!((a - b).norm() < 1e-6);
        }
    }

    #[test]
    fn test_pilot_extraction() {
        let pattern = PilotPattern::new(16, 4);
        let data = vec![Complex32::new(0.0, 0.0); pattern.data_indices.len()];
        let full = pattern.insert(&data);

        let pilots = pattern.extract_pilots(&full);
        assert_eq!(pilots.len(), 4);
        for (_, val) in &pilots {
            assert!((val.re - 1.0).abs() < 1e-6, "Pilot should be +1");
        }
    }

    #[test]
    fn test_coppa_pilot_positions_alternate() {
        let pattern = CoppaPilotPattern::new(48, 4);

        let even = pattern.pilot_indices(0);
        let odd = pattern.pilot_indices(1);

        // Both sets must have the right count.
        assert_eq!(even.len(), 4);
        assert_eq!(odd.len(), 4);

        // Even and odd positions must differ.
        assert_ne!(even, odd, "Even and odd pilot positions should differ");
    }

    #[test]
    fn test_coppa_pilot_pattern_repeats_every_2() {
        let pattern = CoppaPilotPattern::new(48, 4);

        // Symbol 0 and symbol 2 must use the same (even) positions.
        assert_eq!(pattern.pilot_indices(0), pattern.pilot_indices(2));

        // Symbol 1 and symbol 3 must use the same (odd) positions.
        assert_eq!(pattern.pilot_indices(1), pattern.pilot_indices(3));
    }

    #[test]
    fn test_coppa_pilot_insert_extract_roundtrip() {
        // total_carriers = 48, num_pilots = 4 → 44 data carriers.
        let pattern = CoppaPilotPattern::new(48, 4);
        assert_eq!(pattern.num_data(), 44);

        let data: Vec<Complex32> = (0..44)
            .map(|i| Complex32::new(i as f32, -(i as f32)))
            .collect();

        // Test for both even and odd symbols.
        for sym in [0_usize, 1] {
            let full = pattern.insert_pilots(&data, sym);
            assert_eq!(full.len(), 48);

            // Pilot positions should be +1.0.
            for &idx in pattern.pilot_indices(sym) {
                assert!(
                    (full[idx].re - 1.0).abs() < 1e-6 && full[idx].im.abs() < 1e-6,
                    "Pilot at index {idx} should be +1.0 for symbol {sym}"
                );
            }

            // Extracting data should recover the original vector exactly.
            let recovered = pattern.extract_data(&full, sym);
            assert_eq!(recovered.len(), data.len());
            for (i, (orig, got)) in data.iter().zip(recovered.iter()).enumerate() {
                assert!(
                    (orig - got).norm() < 1e-6,
                    "Data mismatch at position {i} for symbol {sym}: orig={orig:?} got={got:?}"
                );
            }
        }
    }
}
