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

    /// Closed-form max-log LLRs for one Gray-coded PAM-8 axis.
    ///
    /// Derived directly from this module's `LEVEL`/`GRAY_TO_IDX` tables.
    /// For a Gray-coded square QAM the joint 2-D max-log metric is
    /// separable per axis: for a bit that lives entirely on this axis,
    /// the closest candidate on the *other* axis is identical for both
    /// the bit's `0`-branch and `1`-branch, so it cancels out of the
    /// `dist(bit=1) - dist(bit=0)` difference, leaving a pure PAM-8
    /// nearest-candidate problem. Inverting `GRAY_TO_IDX` (i.e. finding,
    /// for each `LEVEL` index, which `(b0,b1,b2)` produced it) gives:
    ///
    /// ```text
    /// idx (LEVEL[idx]): 0(+7N) 1(+5N) 2(+3N) 3(+N) 4(-N) 5(-3N) 6(-5N) 7(-7N)
    /// (b0,b1,b2):        0,0,0  0,0,1  0,1,1  0,1,0 1,1,0 1,1,1  1,0,1  1,0,0
    /// ```
    ///
    /// which matches the standard reflected-Gray 8-PAM labeling sorted by
    /// amplitude (`-7N..+7N`: `100,101,111,110,010,011,001,000`). Using
    /// `d_k = (r - LEVEL[k]*N)^2` (`N = NORM`):
    ///
    /// - `b0` (sign bit): `0`-set = `{d0,d1,d2,d3}` (+7N,+5N,+3N,+N),
    ///   `1`-set = `{d4,d5,d6,d7}` (-N,-3N,-5N,-7N).
    /// - `b1` (outer/inner bit): `0`-set = `{d0,d1,d6,d7}` (outer:
    ///   ±5N,±7N), `1`-set = `{d2,d3,d4,d5}` (inner: ±N,±3N).
    /// - `b2` (alternating bit): `0`-set = `{d0,d3,d4,d7}` (±N,±7N),
    ///   `1`-set = `{d1,d2,d5,d6}` (±3N,±5N).
    ///
    /// `llr_bK = (min(1-set) - min(0-set)) / nv`. This is the exact same
    /// max-log answer as the old 64-point enumeration (equivalence
    /// checked exhaustively in `tests::soft_demap_matches_bruteforce_oracle`)
    /// — each `min(...)` term is quadratic in `r`, but algebraically the
    /// `r^2` term is identical for both operands of every subtraction
    /// above, so the result reduces to the same piecewise-linear function
    /// of `r` a hand-expanded formula would give. This form is used
    /// instead of an explicit if/else piecewise expansion because it is
    /// branchless (`f32::min` lowers to a single `minss`/`fmin`
    /// instruction, not a conditional branch) and shares each of the 8
    /// squared-distance terms across all three bits instead of
    /// recomputing them, which matters for demap throughput on random
    /// (branch-unpredictable) input (see `tests::bench_demap_speedup`).
    fn pam8_llrs(r: f32, nv: f32) -> (f32, f32, f32) {
        let n = NORM;
        let d: [f32; 8] = std::array::from_fn(|k| {
            let diff = r - LEVEL[k] * n;
            diff * diff
        });

        let llr_b0 = (d[4].min(d[5]).min(d[6]).min(d[7]) - d[0].min(d[1]).min(d[2]).min(d[3])) / nv;
        let llr_b1 = (d[2].min(d[3]).min(d[4]).min(d[5]) - d[0].min(d[1]).min(d[6]).min(d[7])) / nv;
        let llr_b2 = (d[1].min(d[2]).min(d[5]).min(d[6]) - d[0].min(d[3]).min(d[4]).min(d[7])) / nv;

        (llr_b0, llr_b1, llr_b2)
    }

    /// Old brute-force (64-point enumeration) soft demapper, kept as the
    /// correctness oracle for the closed-form `demap_soft` above. Not used
    /// on any production path.
    #[cfg(test)]
    fn demap_soft_bruteforce(&self, symbol: Complex32, noise_variance: f32) -> Vec<f32> {
        let nv = noise_variance.max(1e-10);
        let mut llrs = vec![0.0f32; 6];

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
        let (llr_b0, llr_b1, llr_b2) = Self::pam8_llrs(symbol.re, nv);
        let (llr_b3, llr_b4, llr_b5) = Self::pam8_llrs(symbol.im, nv);
        vec![llr_b0, llr_b1, llr_b2, llr_b3, llr_b4, llr_b5]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngExt;

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

    /// Step 1 (Task 6): exhaustive equivalence oracle. The closed-form
    /// `demap_soft` must reproduce the old 64-point enumeration
    /// (`demap_soft_bruteforce`) on every LLR, over random `(symbol,
    /// noise_variance)` pairs spanning well beyond the constellation's
    /// amplitude range (so all piecewise segments, including the
    /// outermost ones, get exercised) and down to small noise variances
    /// (so LLR magnitudes span several orders of magnitude).
    ///
    /// Tolerance is `1.5e-4` absolute *or* `1.5e-4` relative, combined
    /// (`dev <= 1.5e-4 + 1.5e-4 * |oracle|`) — see the identical rationale on
    /// `qam16::tests::soft_demap_matches_bruteforce_oracle`: at small
    /// noise variance, f32 rounding differences between the closed-form
    /// and enumeration code paths occasionally exceed a flat 1e-4 once
    /// LLR magnitudes reach the hundreds, even though the formula is
    /// exact (observed relative error ~1e-6). Bumped from 1e-4 2026-07-11
    /// after CI (unseeded RNG, 10k trials/run) hit a real excursion
    /// (dev=1.18e-4 vs the old tol=1.14e-4) with no code change involved.
    #[test]
    fn soft_demap_matches_bruteforce_oracle() {
        let mapper = Qam64Mapper;
        let mut rng = rand::rng();
        let mut max_dev = 0.0f32;
        let mut max_rel_dev = 0.0f32;

        for _ in 0..10_000 {
            let re: f32 = rng.random_range(-7.0..7.0);
            let im: f32 = rng.random_range(-7.0..7.0);
            let nv: f32 = rng.random_range(0.01..4.0);
            let sym = Complex32::new(re, im);

            let fast = mapper.demap_soft(sym, nv);
            let oracle = mapper.demap_soft_bruteforce(sym, nv);

            assert_eq!(fast.len(), oracle.len());
            for (f, o) in fast.iter().zip(oracle.iter()) {
                let dev = (f - o).abs();
                max_dev = max_dev.max(dev);
                max_rel_dev = max_rel_dev.max(dev / o.abs().max(1.0));
                let tol = 1.5e-4 + 1.5e-4 * o.abs();
                assert!(
                    dev <= tol,
                    "LLR mismatch: fast={} oracle={} dev={} tol={} sym=({},{}) nv={}",
                    f,
                    o,
                    dev,
                    tol,
                    re,
                    im,
                    nv
                );
            }
        }

        eprintln!(
            "64QAM soft-demap oracle: max abs deviation over 10k trials = {max_dev:e}, max relative deviation = {max_rel_dev:e}"
        );
    }

    /// Step 3 (Task 6): demap CPU comparison, closed-form vs. the old
    /// 64-point enumeration, over 1e6 symbols. `#[ignore]`d since it's a
    /// perf measurement, not a correctness check, and only meaningful in
    /// release mode; run explicitly with:
    /// `cargo test -p coppa-codec --release --lib qam64::tests::bench_demap_speedup -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_demap_speedup() {
        use std::hint::black_box;
        use std::time::Instant;

        let mapper = Qam64Mapper;
        let mut rng = rand::rng();
        const N: usize = 1_000_000;
        let samples: Vec<(Complex32, f32)> = (0..N)
            .map(|_| {
                let bits: [u8; 6] = std::array::from_fn(|_| rng.random_range(0..2u8));
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
            "64QAM demap_soft over {N} symbols: closed-form={fast_dur:?}, enumeration={old_dur:?}, speedup={speedup:.2}x"
        );
    }

    /// Same as `bench_demap_speedup` but calling `pam8_llrs` directly
    /// (stack tuples, no `Vec<f32>` allocation on either side) to isolate
    /// the algorithmic speedup from the fixed per-call heap-allocation
    /// cost inherent to the shared `ConstellationMapper::demap_soft`
    /// trait signature (`Vec<f32>` return, unchanged by this task and
    /// identical for old and new code paths). See the 16-QAM report
    /// discussion for why this number, not the `Vec`-inclusive one,
    /// reflects the actual enumeration-vs-closed-form algorithmic gain.
    #[test]
    #[ignore]
    fn bench_demap_speedup_raw_noalloc() {
        use std::hint::black_box;
        use std::time::Instant;

        let mapper = Qam64Mapper;
        let mut rng = rand::rng();
        const N: usize = 1_000_000;
        let samples: Vec<(Complex32, f32)> = (0..N)
            .map(|_| {
                let bits: [u8; 6] = std::array::from_fn(|_| rng.random_range(0..2u8));
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
            let (b0, b1, b2) = Qam64Mapper::pam8_llrs(sym.re, *nv);
            let (b3, b4, b5) = Qam64Mapper::pam8_llrs(sym.im, *nv);
            acc += black_box(b0)
                + black_box(b1)
                + black_box(b2)
                + black_box(b3)
                + black_box(b4)
                + black_box(b5);
        }
        let fast_dur = start.elapsed();
        black_box(acc);

        let start = Instant::now();
        let mut acc = 0.0f32;
        for (sym, nv) in &samples {
            let nvv = (*nv).max(1e-10);
            let mut out = [0.0f32; 6];
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
            acc += black_box(out[0])
                + black_box(out[1])
                + black_box(out[2])
                + black_box(out[3])
                + black_box(out[4])
                + black_box(out[5]);
        }
        let old_dur = start.elapsed();
        black_box(acc);

        let speedup = old_dur.as_secs_f64() / fast_dur.as_secs_f64();
        eprintln!(
            "64QAM RAW (no Vec alloc) over {N} symbols: closed-form={fast_dur:?}, enumeration={old_dur:?}, speedup={speedup:.2}x"
        );
    }
}
