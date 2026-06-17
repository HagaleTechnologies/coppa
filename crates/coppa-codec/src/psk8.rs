//! 8PSK constellation mapper with Gray coding.
use crate::traits::ConstellationMapper;
use num_complex::Complex32;
use std::f32::consts::PI;

/// 8PSK constellation with Gray coding.
///
/// 8 points equally spaced on the unit circle, Gray-coded so adjacent
/// points differ by exactly 1 bit.
pub struct Psk8Mapper;

// Gray code ordering for 8PSK: 000, 001, 011, 010, 110, 111, 101, 100
const GRAY_ORDER: [u8; 8] = [0, 1, 3, 2, 6, 7, 5, 4];

impl Psk8Mapper {
    fn constellation_point(index: usize) -> Complex32 {
        let angle = 2.0 * PI * index as f32 / 8.0;
        Complex32::new(angle.cos(), angle.sin())
    }

    fn gray_to_index(gray: u8) -> usize {
        GRAY_ORDER.iter().position(|&g| g == gray).unwrap_or(0)
    }

    fn index_to_gray(index: usize) -> u8 {
        GRAY_ORDER[index % 8]
    }
}

impl ConstellationMapper for Psk8Mapper {
    fn bits_per_symbol(&self) -> usize {
        3
    }

    fn map(&self, bits: &[u8]) -> Complex32 {
        let gray = ((bits[0] & 1) << 2) | ((bits[1] & 1) << 1) | (bits[2] & 1);
        let index = Self::gray_to_index(gray);
        Self::constellation_point(index)
    }

    fn demap_hard(&self, symbol: Complex32) -> Vec<u8> {
        // Find closest constellation point
        let mut min_dist = f32::MAX;
        let mut best_index = 0usize;
        for i in 0..8 {
            let point = Self::constellation_point(i);
            let dist = (symbol - point).norm_sqr();
            if dist < min_dist {
                min_dist = dist;
                best_index = i;
            }
        }
        let gray = Self::index_to_gray(best_index);
        vec![(gray >> 2) & 1, (gray >> 1) & 1, gray & 1]
    }

    fn demap_soft(&self, symbol: Complex32, noise_variance: f32) -> Vec<f32> {
        let nv = noise_variance.max(1e-10);
        let mut llrs = vec![0.0f32; 3];

        // Max-log-MAP: for each bit position, find the closest point
        // with bit=0 and the closest point with bit=1
        for (bit_pos, llr) in llrs.iter_mut().enumerate() {
            let mut min_dist_0 = f32::MAX;
            let mut min_dist_1 = f32::MAX;

            for i in 0..8 {
                let gray = Self::index_to_gray(i);
                let bit_val = (gray >> (2 - bit_pos)) & 1;
                let point = Self::constellation_point(i);
                let dist = (symbol - point).norm_sqr();

                if bit_val == 0 {
                    min_dist_0 = min_dist_0.min(dist);
                } else {
                    min_dist_1 = min_dist_1.min(dist);
                }
            }

            // LLR = (min_dist_1 - min_dist_0) / noise_variance
            *llr = (min_dist_1 - min_dist_0) / nv;
        }

        llrs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_8psk_roundtrip() {
        let mapper = Psk8Mapper;
        for i in 0..8u8 {
            let bits = vec![(i >> 2) & 1, (i >> 1) & 1, i & 1];
            let sym = mapper.map(&bits);
            let demapped = mapper.demap_hard(sym);
            assert_eq!(demapped, bits, "Failed for bits {:?}", bits);
        }
    }

    #[test]
    fn test_8psk_unit_power() {
        let mapper = Psk8Mapper;
        for i in 0..8u8 {
            let bits = vec![(i >> 2) & 1, (i >> 1) & 1, i & 1];
            let sym = mapper.map(&bits);
            let power = sym.norm_sqr();
            assert!(
                (power - 1.0).abs() < 1e-5,
                "8PSK symbols should have unit power, got {}",
                power
            );
        }
    }

    #[test]
    fn test_8psk_gray_coding() {
        // Adjacent constellation points should differ by exactly 1 bit
        for i in 0..8 {
            let next = (i + 1) % 8;
            let g1 = Psk8Mapper::index_to_gray(i);
            let g2 = Psk8Mapper::index_to_gray(next);
            let diff = (g1 ^ g2).count_ones();
            assert_eq!(
                diff, 1,
                "Adjacent 8PSK points should differ by 1 bit: {} vs {}",
                g1, g2
            );
        }
    }

    #[test]
    fn test_8psk_soft_demap() {
        let mapper = Psk8Mapper;
        // A point exactly at constellation index 0 (angle=0, gray=000)
        let sym = Complex32::new(1.0, 0.0);
        let llr = mapper.demap_soft(sym, 0.1);
        // All bits should be 0 -> all LLRs positive
        assert!(llr[0] > 0.0, "LLR[0] should be positive, got {}", llr[0]);
        assert!(llr[1] > 0.0, "LLR[1] should be positive, got {}", llr[1]);
        assert!(llr[2] > 0.0, "LLR[2] should be positive, got {}", llr[2]);
    }
}
