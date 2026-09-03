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

    /// Closed-form max-log LLRs for one Gray-coded PAM-4 axis.
    ///
    /// Derived directly from this module's `LEVEL` table (`[3, 1, -1, -3] *
    /// NORM`, indexed by `idx = (b0 << 1) | b1`). For a Gray-coded square
    /// QAM the joint 2-D max-log metric is separable per axis: for a bit
    /// that lives entirely on this axis, the closest candidate on the
    /// *other* axis is identical for both the bit's `0`-branch and
    /// `1`-branch, so it cancels out of the `dist(bit=1) - dist(bit=0)`
    /// difference. That leaves a pure PAM-4 nearest-candidate problem,
    /// with exactly the candidates below (using `d_k = (r - LEVEL[k]*N)^2`,
    /// `N = NORM`):
    ///
    /// ```text
    /// idx:        0     1     2     3
    /// level:     +3N   +N    -N   -3N
    /// (b0,b1):   0,0   0,1   1,0   1,1
    /// ```
    ///
    /// - `b0` (sign bit): `0`-set = `{d0,d1}` (+3N,+N), `1`-set = `{d2,d3}`
    ///   (-N,-3N). `llr_b0 = (min(d2,d3) - min(d0,d1)) / nv`.
    /// - `b1` (alternating bit): `0`-set = `{d0,d2}` (+3N,-N), `1`-set =
    ///   `{d1,d3}` (+N,-3N). `llr_b1 = (min(d1,d3) - min(d0,d2)) / nv`.
    ///
    /// This is the exact same max-log answer as the old 16-point
    /// enumeration (equivalence checked exhaustively in
    /// `tests::soft_demap_matches_bruteforce_oracle`) — each `min(...)`
    /// term is quadratic in `r`, but algebraically the `r^2` term is
    /// identical for both operands of every subtraction above, so the
    /// result is piecewise-linear in `r` with breakpoints at the
    /// midpoints between adjacent candidate levels, same as a
    /// hand-expanded piecewise formula would give — this form is used
    /// instead because it is branchless (`f32::min` lowers to a single
    /// `minss`/`fmin` instruction, not a conditional branch) and shares
    /// each of the 4 squared-distance terms across both bits instead of
    /// recomputing them, which matters for demap throughput (see
    /// `tests::bench_demap_speedup`).
    fn pam4_llrs(r: f32, nv: f32) -> (f32, f32) {
        let n = NORM;
        let d0 = (r - 3.0 * n) * (r - 3.0 * n); // level +3N: (b0,b1) = (0,0)
        let d1 = (r - n) * (r - n); //             level +N:  (b0,b1) = (0,1)
        let d2 = (r + n) * (r + n); //             level -N:  (b0,b1) = (1,0)
        let d3 = (r + 3.0 * n) * (r + 3.0 * n); // level -3N: (b0,b1) = (1,1)

        let llr_b0 = (d2.min(d3) - d0.min(d1)) / nv;
        let llr_b1 = (d1.min(d3) - d0.min(d2)) / nv;
        (llr_b0, llr_b1)
    }

    /// Old brute-force (16-point enumeration) soft demapper, kept as the
    /// correctness oracle for the closed-form `demap_soft` above. Not used
    /// on any production path.
    #[cfg(test)]
    fn demap_soft_bruteforce(&self, symbol: Complex32, noise_variance: f32) -> Vec<f32> {
        let nv = noise_variance.max(1e-10);
        let mut llrs = vec![0.0f32; 4];

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
        let (llr_b0, llr_b1) = Self::pam4_llrs(symbol.re, nv);
        let (llr_b2, llr_b3) = Self::pam4_llrs(symbol.im, nv);
        vec![llr_b0, llr_b1, llr_b2, llr_b3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

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

    /// Largest unnormalized level magnitude in [`LEVEL`], derived from the
    /// mapper's own table rather than duplicated as a literal so a future
    /// constellation change can't silently desync the oracle tolerance model.
    fn level_lmax() -> f32 {
        LEVEL.iter().copied().fold(f32::MIN, f32::max) * NORM
    }

    /// COP-7: QAM-16 has the same cancellation mechanism as QAM-64 (see
    /// `qam_oracle_tol`), just 6x more headroom because its draw range caps Dmax at
    /// ~31.18 rather than ~64.
    #[test]
    fn oracle_tolerance_covers_worst_case_conditioning() {
        let lmax = level_lmax();
        let sym = Complex32::new(3.0, 3.0);
        let dmax = crate::qam_oracle_tol::worst_case_dmax(sym, lmax);

        assert!(
            (dmax - 31.18).abs() < 0.01,
            "expected worst-case Dmax ~= 31.18, got {dmax}"
        );
    }

    /// COP-8 round 1: the symmetric corner `(3.0, 3.0)` above turns out to
    /// produce bit-identical closed-form/oracle LLRs (Codex-confirmed on
    /// PR #95) -- both paths round identically for that input, so a
    /// deviation check pinned there is vacuous and would pass even with the
    /// `K * EPS * Dmax / nv` term deleted entirely. This uses a concrete
    /// input found by a targeted search near the same worst-case-
    /// conditioning corner (re/im in `2.0..3.5`, nv in `0.005..0.05`,
    /// scratch search not committed, seed `0xC0FFEE`) that DOES exhibit
    /// nonzero f32 cancellation residual, so the coverage assertion below
    /// genuinely depends on the codebase's own tolerance model rather than
    /// restating a margin constant.
    #[test]
    fn oracle_tolerance_covers_a_measured_worst_case_residual() {
        let mapper = Qam16Mapper;
        let re = 3.166_981_7_f32;
        let im = 3.493_435_1_f32;
        let nv = 0.005_015_685_f32;
        let sym = Complex32::new(re, im);
        let dmax = crate::qam_oracle_tol::worst_case_dmax(sym, level_lmax());

        let fast = mapper.demap_soft(sym, nv);
        let oracle = mapper.demap_soft_bruteforce(sym, nv);
        let mut max_dev = 0.0f32;
        for (f, o) in fast.iter().zip(oracle.iter()) {
            let dev = (f - o).abs();
            max_dev = max_dev.max(dev);
            let tol = crate::qam_oracle_tol::oracle_tol(*o, dmax, nv);
            assert!(
                dev <= tol,
                "measured deviation {dev:e} exceeds tolerance {tol:e} at \
                 sym=({re},{im}) nv={nv}"
            );
        }
        assert!(
            max_dev > 0.0,
            "expected this fixture to exercise real f32 cancellation \
             (nonzero deviation); if it now reads zero, the demapper \
             implementation changed and this fixture needs replacing with a \
             fresh nonzero-residual input"
        );
    }

    /// Step 1 (Task 6): exhaustive equivalence oracle. The closed-form
    /// `demap_soft` must reproduce the old 16-point enumeration
    /// (`demap_soft_bruteforce`) on every LLR, over random `(symbol,
    /// noise_variance)` pairs spanning well beyond the constellation's
    /// amplitude range (so all piecewise segments, incl. the outer ones,
    /// get exercised) and down to small noise variances (so LLR
    /// magnitudes span several orders of magnitude).
    ///
    /// The condition-aware tolerance is derived in `qam_oracle_tol`. It retains
    /// the absolute-plus-relative bound bumped from 1e-4 on 2026-07-11, and
    /// COP-7 adds the `Dmax / nv` term required by the oracle's cancellation.
    #[test]
    fn soft_demap_matches_bruteforce_oracle() {
        let mapper = Qam16Mapper;
        let seed: u64 = std::env::var("QAM_ORACLE_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| rand::rng().random());
        eprintln!("16QAM soft-demap oracle: seed={seed} (re-run with QAM_ORACLE_SEED={seed})");
        let mut rng = StdRng::seed_from_u64(seed);
        let mut max_dev = 0.0f32;
        let mut max_rel_dev = 0.0f32;

        for _ in 0..10_000 {
            let re: f32 = rng.random_range(-3.0..3.0);
            let im: f32 = rng.random_range(-3.0..3.0);
            let nv: f32 = rng.random_range(0.01..4.0);
            let sym = Complex32::new(re, im);

            let fast = mapper.demap_soft(sym, nv);
            let oracle = mapper.demap_soft_bruteforce(sym, nv);
            let dmax = crate::qam_oracle_tol::worst_case_dmax(sym, level_lmax());

            assert_eq!(fast.len(), oracle.len());
            for (f, o) in fast.iter().zip(oracle.iter()) {
                let dev = (f - o).abs();
                max_dev = max_dev.max(dev);
                max_rel_dev = max_rel_dev.max(dev / o.abs().max(1.0));
                let tol = crate::qam_oracle_tol::oracle_tol(*o, dmax, nv);
                assert!(
                    dev <= tol,
                    "LLR mismatch: fast={f} oracle={o} dev={dev:e} tol={tol:e} \
                     sym=({re},{im}) nv={nv} dmax={dmax} eps*dmax/nv={:e} seed={seed}",
                    crate::qam_oracle_tol::EPS * dmax / nv
                );
            }
        }

        eprintln!(
            "16QAM soft-demap oracle: max abs deviation over 10k trials = {max_dev:e}, max relative deviation = {max_rel_dev:e}"
        );
    }

    /// Step 3 (Task 6): demap CPU comparison, closed-form vs. the old
    /// 16-point enumeration, over 1e6 symbols. `#[ignore]`d since it's a
    /// perf measurement, not a correctness check, and only meaningful in
    /// release mode; run explicitly with:
    /// `cargo test -p coppa-codec --release --lib qam16::tests::bench_demap_speedup -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_demap_speedup() {
        use std::hint::black_box;
        use std::time::Instant;

        let mapper = Qam16Mapper;
        let mut rng = rand::rng();
        const N: usize = 1_000_000;
        let samples: Vec<(Complex32, f32)> = (0..N)
            .map(|_| {
                let bits = [
                    rng.random_range(0..2u8),
                    rng.random_range(0..2u8),
                    rng.random_range(0..2u8),
                    rng.random_range(0..2u8),
                ];
                let clean = mapper.map(&bits);
                let nv: f32 = rng.random_range(0.02..0.5);
                let sigma = (nv / 2.0).sqrt();
                let u1: f32 = rng.random_range(1e-6..1.0);
                let u2: f32 = rng.random_range(0.0..std::f32::consts::TAU);
                let r = (-2.0 * u1.ln()).sqrt();
                let noise = Complex32::new(r * u2.cos() * sigma, r * u2.sin() * sigma);
                (clean + noise, nv)
            })
            .collect();

        let start = Instant::now();
        let mut acc = 0.0f32;
        for (sym, nv) in &samples {
            let llr = mapper.demap_soft(*sym, *nv);
            acc += llr[0];
        }
        let fast_dur = start.elapsed();
        black_box(acc);

        let start = Instant::now();
        let mut acc = 0.0f32;
        for (sym, nv) in &samples {
            let llr = mapper.demap_soft_bruteforce(*sym, *nv);
            acc += llr[0];
        }
        let old_dur = start.elapsed();
        black_box(acc);

        let speedup = old_dur.as_secs_f64() / fast_dur.as_secs_f64();
        eprintln!(
            "16QAM demap_soft over {N} symbols: closed-form={fast_dur:?}, enumeration={old_dur:?}, speedup={speedup:.2}x"
        );
    }

    #[test]
    #[ignore]
    fn bench_demap_speedup_raw_noalloc() {
        use std::hint::black_box;
        use std::time::Instant;

        let mapper = Qam16Mapper;
        let mut rng = rand::rng();
        const N: usize = 1_000_000;
        let samples: Vec<(Complex32, f32)> = (0..N)
            .map(|_| {
                let bits = [
                    rng.random_range(0..2u8),
                    rng.random_range(0..2u8),
                    rng.random_range(0..2u8),
                    rng.random_range(0..2u8),
                ];
                let clean = mapper.map(&bits);
                let nv: f32 = rng.random_range(0.02..0.5);
                let sigma = (nv / 2.0).sqrt();
                let u1: f32 = rng.random_range(1e-6..1.0);
                let u2: f32 = rng.random_range(0.0..std::f32::consts::TAU);
                let r = (-2.0 * u1.ln()).sqrt();
                let noise = Complex32::new(r * u2.cos() * sigma, r * u2.sin() * sigma);
                (clean + noise, nv)
            })
            .collect();

        let start = Instant::now();
        let mut acc = 0.0f32;
        for (sym, nv) in &samples {
            let (b0, b1) = Qam16Mapper::pam4_llrs(sym.re, *nv);
            let (b2, b3) = Qam16Mapper::pam4_llrs(sym.im, *nv);
            acc += black_box(b0) + black_box(b1) + black_box(b2) + black_box(b3);
        }
        let fast_dur = start.elapsed();
        black_box(acc);

        let start = Instant::now();
        let mut acc = 0.0f32;
        for (sym, nv) in &samples {
            let llrs_bruteforce_raw = {
                let nvv = (*nv).max(1e-10);
                let mut out = [0.0f32; 4];
                for bit_pos in 0..4 {
                    let mut min_dist_0 = f32::MAX;
                    let mut min_dist_1 = f32::MAX;
                    for idx in 0..16u8 {
                        let bits = [(idx >> 3) & 1, (idx >> 2) & 1, (idx >> 1) & 1, idx & 1];
                        let point = mapper.map(&bits);
                        let dist = (*sym - point).norm_sqr();
                        if bits[bit_pos] == 0 {
                            min_dist_0 = min_dist_0.min(dist);
                        } else {
                            min_dist_1 = min_dist_1.min(dist);
                        }
                    }
                    out[bit_pos] = (min_dist_1 - min_dist_0) / nvv;
                }
                out
            };
            acc += black_box(llrs_bruteforce_raw[0])
                + black_box(llrs_bruteforce_raw[1])
                + black_box(llrs_bruteforce_raw[2])
                + black_box(llrs_bruteforce_raw[3]);
        }
        let old_dur = start.elapsed();
        black_box(acc);

        let speedup = old_dur.as_secs_f64() / fast_dur.as_secs_f64();
        eprintln!(
            "16QAM RAW (no Vec alloc) over {N} symbols: closed-form={fast_dur:?}, enumeration={old_dur:?}, speedup={speedup:.2}x"
        );
    }
}
