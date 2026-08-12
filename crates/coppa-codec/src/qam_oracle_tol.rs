//! Tolerance model shared by the QAM-16 / QAM-64 soft-demapper oracle tests.
//!
//! The closed-form path is purely 1-D per axis, while the brute-force oracle
//! forms full 2-D squared distances before subtracting them. In f32 the shared
//! other-axis term can therefore lose low-order bits to cancellation, with the
//! residual amplified by small noise variance.
//!
//! Across 240M LLR comparisons (COP-7 research), deviation was bounded by
//! `C * EPS * Dmax / nv` with a maximum observed `C` of 1.789. `K = 4` gives
//! roughly 2.2x margin while preserving a tight bound in benign regions.

use num_complex::Complex32;

/// f32 machine epsilon (`2^-24`), the per-operation relative rounding bound.
pub(crate) const EPS: f32 = 5.960_464_5e-8;

/// Margin over the worst observed conditioning coefficient (1.789).
const K: f32 = 4.0;

/// Largest squared distance from `symbol` to any square-QAM constellation point.
pub(crate) fn worst_case_dmax(symbol: Complex32, lmax: f32) -> f32 {
    let dr = symbol.re.abs() + lmax;
    let di = symbol.im.abs() + lmax;
    dr * dr + di * di
}

/// Permitted deviation between the closed-form and enumeration demappers.
pub(crate) fn oracle_tol(oracle: f32, dmax: f32, nv: f32) -> f32 {
    1.5e-4 + 1.5e-4 * oracle.abs() + K * EPS * dmax / nv.max(1e-10)
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex32;

    #[test]
    fn dmax_is_the_opposite_corner() {
        let lmax = 3.0 * 0.316_227_8_f32;
        let sym = Complex32::new(1.0, 2.0);
        let expected = (1.0 + lmax).powi(2) + (2.0 + lmax).powi(2);
        assert!((worst_case_dmax(sym, lmax) - expected).abs() < 1e-5);
        assert_eq!(
            worst_case_dmax(sym, lmax),
            worst_case_dmax(Complex32::new(-1.0, -2.0), lmax)
        );
    }

    #[test]
    fn tolerance_grows_as_conditioning_worsens() {
        let loose = oracle_tol(0.5, 64.0, 0.01);
        let tight = oracle_tol(0.5, 64.0, 4.0);
        assert!(loose > tight, "tol must scale with 1/nv: {loose} vs {tight}");
        assert!(loose > EPS * 64.0 / 0.01);
    }

    #[test]
    fn tolerance_stays_tight_where_conditioning_is_benign() {
        let tol = oracle_tol(1000.0, 64.0, 4.0);
        assert!(tol < 0.2, "benign-region tolerance drifted loose: {tol}");
    }

    #[test]
    fn sweep_seed_is_reproducible() {
        use rand::rngs::StdRng;
        use rand::{RngExt, SeedableRng};

        let draw = |seed: u64| {
            let mut rng = StdRng::seed_from_u64(seed);
            (0..8)
                .map(|_| rng.random_range(-7.0..7.0f32))
                .collect::<Vec<_>>()
        };
        assert_eq!(draw(0xC0F7), draw(0xC0F7), "same seed must replay exactly");
        assert_ne!(draw(1), draw(2), "different seeds must diverge");
    }
}
