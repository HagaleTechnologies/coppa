//! QPSK (Quadrature Phase Shift Keying) constellation mapper with Gray coding.
use crate::traits::ConstellationMapper;
use num_complex::Complex32;

/// QPSK constellation with Gray coding.
///
/// Standard Gray-coded QPSK (angular adjacency differs by 1 bit):
///   00 -> (+1, +1) / sqrt(2)   (45 degrees)
///   01 -> (-1, +1) / sqrt(2)   (135 degrees)
///   11 -> (-1, -1) / sqrt(2)   (225 degrees)
///   10 -> (+1, -1) / sqrt(2)   (315 degrees)
///
/// b0 is determined by sign of imaginary part (0=positive, 1=negative)
/// b1 is determined by sign of real part (0=positive, 1=negative)
///
/// Mapping a pair of bits and hard-demapping the resulting symbol is a
/// roundtrip:
///
/// ```
/// use coppa_codec::qpsk::QpskMapper;
/// use coppa_codec::traits::ConstellationMapper;
///
/// let mapper = QpskMapper;
/// for bits in [[0u8, 0], [0, 1], [1, 0], [1, 1]] {
///     let symbol = mapper.map(&bits);
///     assert_eq!(mapper.demap_hard(symbol), bits.to_vec());
/// }
/// ```
pub struct QpskMapper;

const SCALE: f32 = std::f32::consts::FRAC_1_SQRT_2;

// Gray-coded constellation: index = (b0 << 1) | b1
// Ordered so angular neighbors differ by 1 bit
const CONSTELLATION: [(f32, f32); 4] = [
    (SCALE, SCALE),   // 00: Q1
    (-SCALE, SCALE),  // 01: Q2
    (SCALE, -SCALE),  // 10: Q4
    (-SCALE, -SCALE), // 11: Q3
];

impl ConstellationMapper for QpskMapper {
    fn bits_per_symbol(&self) -> usize {
        2
    }

    fn map(&self, bits: &[u8]) -> Complex32 {
        let idx = ((bits[0] & 1) << 1) | (bits[1] & 1);
        let (re, im) = CONSTELLATION[idx as usize];
        Complex32::new(re, im)
    }

    fn demap_hard(&self, symbol: Complex32) -> Vec<u8> {
        // b0 from imaginary sign, b1 from real sign
        let b0 = if symbol.im >= 0.0 { 0u8 } else { 1 };
        let b1 = if symbol.re >= 0.0 { 0u8 } else { 1 };
        vec![b0, b1]
    }

    fn demap_soft(&self, symbol: Complex32, noise_variance: f32) -> Vec<f32> {
        let nv = noise_variance.max(1e-10);
        // Exact max-log LLR per axis (decision 8): QPSK is two independent
        // orthogonal BPSK sub-channels, each with per-axis amplitude
        // `SCALE = 1/sqrt(2)`. Substituting that amplitude into the exact
        // max-log BPSK scale (`4 * a * component / sigma^2`, see
        // `bpsk::BpskMapper::demap_soft`) gives `4/sqrt(2) = 2*sqrt(2)` per axis.
        // LLR(b0) from imaginary, LLR(b1) from real. Positive LLR = more likely bit 0.
        const LLR_SCALE: f32 = 2.0 * std::f32::consts::SQRT_2;
        vec![LLR_SCALE * symbol.im / nv, LLR_SCALE * symbol.re / nv]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qpsk_roundtrip() {
        let mapper = QpskMapper;
        for b0 in 0..2u8 {
            for b1 in 0..2u8 {
                let sym = mapper.map(&[b0, b1]);
                let demapped = mapper.demap_hard(sym);
                assert_eq!(demapped, vec![b0, b1], "Failed for bits [{}, {}]", b0, b1);
            }
        }
    }

    #[test]
    fn test_qpsk_unit_power() {
        let mapper = QpskMapper;
        for b0 in 0..2u8 {
            for b1 in 0..2u8 {
                let sym = mapper.map(&[b0, b1]);
                let power = sym.norm_sqr();
                assert!(
                    (power - 1.0).abs() < 1e-6,
                    "QPSK symbols should have unit power, got {}",
                    power
                );
            }
        }
    }

    #[test]
    fn test_qpsk_soft_demap_signs() {
        let mapper = QpskMapper;
        // Point in first quadrant -> both bits should be 0 (positive LLRs)
        let llr = mapper.demap_soft(Complex32::new(0.5, 0.5), 1.0);
        assert!(llr[0] > 0.0);
        assert!(llr[1] > 0.0);

        // Point in third quadrant -> both bits should be 1 (negative LLRs)
        let llr = mapper.demap_soft(Complex32::new(-0.5, -0.5), 1.0);
        assert!(llr[0] < 0.0);
        assert!(llr[1] < 0.0);
    }

    /// Exact max-log QPSK LLR scale (decision 8): `2*sqrt(2) * component / sigma^2`
    /// per axis.
    #[test]
    fn test_qpsk_soft_demap_exact_max_log_scale() {
        let mapper = QpskMapper;
        let expected = 2.0 * std::f32::consts::SQRT_2 * 1.0 / 0.5;
        let llr = mapper.demap_soft(Complex32::new(1.0, 1.0), 0.5);
        assert!(
            (llr[0] - expected).abs() < 1e-4,
            "LLR(b0) should be 2*sqrt(2)*1.0/0.5 = {}, got {}",
            expected,
            llr[0]
        );
        assert!(
            (llr[1] - expected).abs() < 1e-4,
            "LLR(b1) should be 2*sqrt(2)*1.0/0.5 = {}, got {}",
            expected,
            llr[1]
        );
    }

    #[test]
    fn test_qpsk_gray_coding() {
        // Adjacent constellation points (in angle) should differ by exactly 1 bit
        // Angular order: 00 (45°), 01 (135°), 11 (225°), 10 (315°)
        let mapper = QpskMapper;
        let angular_order: Vec<Vec<u8>> = vec![
            vec![0, 0], // 45°
            vec![0, 1], // 135°
            vec![1, 1], // 225°
            vec![1, 0], // 315°
        ];

        for i in 0..4 {
            let next = (i + 1) % 4;
            let diff: usize = angular_order[i]
                .iter()
                .zip(angular_order[next].iter())
                .map(|(a, b)| if a != b { 1 } else { 0 })
                .sum();
            assert_eq!(diff, 1, "Adjacent QPSK points should differ by 1 bit");
        }

        // Verify roundtrip for all angular order points
        for bits in &angular_order {
            let sym = mapper.map(bits);
            let demapped = mapper.demap_hard(sym);
            assert_eq!(&demapped, bits);
        }
    }
}
