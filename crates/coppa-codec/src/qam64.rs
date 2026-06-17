//! 64QAM constellation mapper with Gray coding.
use crate::traits::ConstellationMapper;
use num_complex::Complex32;

/// 64QAM constellation with Gray coding.
///
/// 8x8 grid with Gray-coded I and Q axes, normalized to unit average power.
/// I axis uses bits [b0, b1, b2], Q axis uses bits [b3, b4, b5].
pub struct Qam64Mapper;

// Normalized amplitude for 64QAM
// Average power (unnormalized ±1,±3,±5,±7) = 2*(1+9+25+49)/8 = 42/4 = 21 per axis
// Total = 42, scale = 1/sqrt(42)
const NORM: f32 = 0.154_303_35; // 1/sqrt(42)

// Gray-coded 3-bit to amplitude mapping
// 000->+7, 001->+5, 011->+3, 010->+1, 110->-1, 111->-3, 101->-5, 100->-7
const LEVEL: [f32; 8] = [7.0, 5.0, 3.0, 1.0, -1.0, -3.0, -5.0, -7.0];

// Gray code table for 3 bits
const GRAY_TO_IDX: [usize; 8] = [0, 1, 3, 2, 7, 6, 4, 5];

impl Qam64Mapper {
    fn bits_to_level(b0: u8, b1: u8, b2: u8) -> f32 {
        let gray = ((b0 & 1) << 2) | ((b1 & 1) << 1) | (b2 & 1);
        let idx = GRAY_TO_IDX[gray as usize];
        LEVEL[idx] * NORM
    }

    fn level_to_bits(val: f32) -> (u8, u8, u8) {
        let unnormed = val / NORM;
        let idx = LEVEL
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = (unnormed - *a).abs();
                let db = (unnormed - *b).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap()
            .0;
        // Reverse lookup: find which Gray code maps to this index
        let gray = GRAY_TO_IDX.iter().position(|&g| g == idx).unwrap() as u8;
        ((gray >> 2) & 1, (gray >> 1) & 1, gray & 1)
    }
}

impl ConstellationMapper for Qam64Mapper {
    fn bits_per_symbol(&self) -> usize {
        6
    }

    fn map(&self, bits: &[u8]) -> Complex32 {
        let re = Self::bits_to_level(bits[0], bits[1], bits[2]);
        let im = Self::bits_to_level(bits[3], bits[4], bits[5]);
        Complex32::new(re, im)
    }

    fn demap_hard(&self, symbol: Complex32) -> Vec<u8> {
        let (b0, b1, b2) = Self::level_to_bits(symbol.re);
        let (b3, b4, b5) = Self::level_to_bits(symbol.im);
        vec![b0, b1, b2, b3, b4, b5]
    }

    fn demap_soft(&self, symbol: Complex32, noise_variance: f32) -> Vec<f32> {
        let nv = noise_variance.max(1e-10);
        let mut llrs = vec![0.0f32; 6];

        // Max-log-MAP: enumerate all 64 constellation points
        for bit_pos in 0..6 {
            let mut min_dist_0 = f32::MAX;
            let mut min_dist_1 = f32::MAX;

            for idx in 0..64u8 {
                let bits = [
                    (idx >> 5) & 1,
                    (idx >> 4) & 1,
                    (idx >> 3) & 1,
                    (idx >> 2) & 1,
                    (idx >> 1) & 1,
                    idx & 1,
                ];
                let point = self.map(&bits);
                let dist = (symbol - point).norm_sqr();

                if bits[bit_pos] == 0 {
                    min_dist_0 = min_dist_0.min(dist);
                } else {
                    min_dist_1 = min_dist_1.min(dist);
                }
            }

            llrs[bit_pos] = (min_dist_1 - min_dist_0) / nv;
        }

        llrs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_64qam_roundtrip() {
        let mapper = Qam64Mapper;
        for i in 0..64u8 {
            let bits = vec![
                (i >> 5) & 1,
                (i >> 4) & 1,
                (i >> 3) & 1,
                (i >> 2) & 1,
                (i >> 1) & 1,
                i & 1,
            ];
            let sym = mapper.map(&bits);
            let demapped = mapper.demap_hard(sym);
            assert_eq!(demapped, bits, "Failed for bits {:?} (idx {})", bits, i);
        }
    }

    #[test]
    fn test_64qam_average_power() {
        let mapper = Qam64Mapper;
        let mut total_power = 0.0f32;
        for i in 0..64u8 {
            let bits = vec![
                (i >> 5) & 1,
                (i >> 4) & 1,
                (i >> 3) & 1,
                (i >> 2) & 1,
                (i >> 1) & 1,
                i & 1,
            ];
            let sym = mapper.map(&bits);
            total_power += sym.norm_sqr();
        }
        let avg_power = total_power / 64.0;
        assert!(
            (avg_power - 1.0).abs() < 0.01,
            "64QAM should have unit average power, got {}",
            avg_power
        );
    }

    #[test]
    fn test_64qam_soft_demap() {
        let mapper = Qam64Mapper;
        let sym = mapper.map(&[0, 0, 0, 0, 0, 0]);
        let llr = mapper.demap_soft(sym, 0.1);
        for (i, &l) in llr.iter().enumerate() {
            assert!(
                l > 0.0,
                "LLR[{}] should be positive for all-zero bits, got {}",
                i,
                l
            );
        }
    }
}
