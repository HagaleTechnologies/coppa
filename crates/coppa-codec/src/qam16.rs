//! 16QAM constellation mapper with Gray coding.
use crate::traits::ConstellationMapper;
use num_complex::Complex32;

/// 16QAM constellation with Gray coding.
///
/// 4x4 grid with Gray-coded I and Q axes, normalized to unit average power.
/// I axis uses bits [b0, b1], Q axis uses bits [b2, b3].
pub struct Qam16Mapper;

// Normalized amplitude levels for 16QAM (unit average power)
// Average power = (2 * (1^2 + 3^2)) / 4 = 10/4 = 2.5 per axis
// Total average power = 2 * 2.5 = 5.0 (for unnormalized ±1, ±3)
// Scale factor = 1/sqrt(10) to normalize to unit power
const NORM: f32 = 0.316_227_8; // 1/sqrt(10)

// Gray-coded 2-bit to amplitude mapping
// 00 -> +3, 01 -> +1, 11 -> -1, 10 -> -3
const LEVEL: [f32; 4] = [3.0, 1.0, -1.0, -3.0];

impl Qam16Mapper {
    fn bits_to_level(b0: u8, b1: u8) -> f32 {
        let idx = ((b0 & 1) << 1) | (b1 & 1);
        LEVEL[idx as usize] * NORM
    }

    fn level_to_bits(val: f32) -> (u8, u8) {
        // Find closest level
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
        ((idx >> 1) as u8 & 1, idx as u8 & 1)
    }
}

impl ConstellationMapper for Qam16Mapper {
    fn bits_per_symbol(&self) -> usize {
        4
    }

    fn map(&self, bits: &[u8]) -> Complex32 {
        let re = Self::bits_to_level(bits[0], bits[1]);
        let im = Self::bits_to_level(bits[2], bits[3]);
        Complex32::new(re, im)
    }

    fn demap_hard(&self, symbol: Complex32) -> Vec<u8> {
        let (b0, b1) = Self::level_to_bits(symbol.re);
        let (b2, b3) = Self::level_to_bits(symbol.im);
        vec![b0, b1, b2, b3]
    }

    fn demap_soft(&self, symbol: Complex32, noise_variance: f32) -> Vec<f32> {
        let nv = noise_variance.max(1e-10);
        let mut llrs = vec![0.0f32; 4];

        // Max-log-MAP: enumerate all 16 constellation points
        for bit_pos in 0..4 {
            let mut min_dist_0 = f32::MAX;
            let mut min_dist_1 = f32::MAX;

            for idx in 0..16u8 {
                let bits = [(idx >> 3) & 1, (idx >> 2) & 1, (idx >> 1) & 1, idx & 1];
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
    fn test_16qam_roundtrip() {
        let mapper = Qam16Mapper;
        for i in 0..16u8 {
            let bits = vec![(i >> 3) & 1, (i >> 2) & 1, (i >> 1) & 1, i & 1];
            let sym = mapper.map(&bits);
            let demapped = mapper.demap_hard(sym);
            assert_eq!(demapped, bits, "Failed for bits {:?}", bits);
        }
    }

    #[test]
    fn test_16qam_average_power() {
        let mapper = Qam16Mapper;
        let mut total_power = 0.0f32;
        for i in 0..16u8 {
            let bits = vec![(i >> 3) & 1, (i >> 2) & 1, (i >> 1) & 1, i & 1];
            let sym = mapper.map(&bits);
            total_power += sym.norm_sqr();
        }
        let avg_power = total_power / 16.0;
        assert!(
            (avg_power - 1.0).abs() < 0.01,
            "16QAM should have unit average power, got {}",
            avg_power
        );
    }

    #[test]
    fn test_16qam_soft_demap() {
        let mapper = Qam16Mapper;
        // Point at (3, 3)/sqrt(10) = bits 0000
        let sym = mapper.map(&[0, 0, 0, 0]);
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
