//! COP-170: per-cell statistics. The cell value is the mean net bytes/min over
//! every trial, failures included, with a two-sided 95% Student t interval
//! clamped at 0. Completion uses [`crate::metrics::wilson_ci95`].

use crate::metrics::wilson_ci95;

/// 0.975 quantiles of Student's t for 1 to 30 degrees of freedom.
const T975_1_TO_30: [f64; 30] = [
    12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160,
    2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056,
    2.052, 2.048, 2.045, 2.042,
];

/// Tabulated quantiles beyond 30 degrees of freedom: (df, t).
const T975_TAIL: [(f64, f64); 3] = [(40.0, 2.021), (60.0, 2.000), (120.0, 1.980)];

/// The normal limit.
const Z975: f64 = 1.959_964;

/// The 0.975 quantile of Student's t with `df` degrees of freedom. Values
/// between table rows are interpolated linearly in 1/df. `df = 0` returns
/// infinity.
#[must_use]
pub fn t975(df: usize) -> f64 {
    if df == 0 {
        return f64::INFINITY;
    }
    if df <= 30 {
        return T975_1_TO_30[df - 1];
    }
    let x = 1.0 / df as f64;
    let mut prev = (30.0, T975_1_TO_30[29]);
    for &(d, t) in T975_TAIL.iter() {
        if (df as f64) <= d {
            return interpolate(prev, (d, t), x);
        }
        prev = (d, t);
    }
    // Between df 120 and infinity (1/df = 0).
    let (d0, t0) = prev;
    let x0 = 1.0 / d0;
    Z975 + (t0 - Z975) * (x / x0)
}

fn interpolate((d0, t0): (f64, f64), (d1, t1): (f64, f64), x: f64) -> f64 {
    let (x0, x1) = (1.0 / d0, 1.0 / d1);
    t0 + (t1 - t0) * (x - x0) / (x1 - x0)
}

/// A mean with its 95% interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeanCi {
    /// Number of values.
    pub n: usize,
    /// Sample mean.
    pub mean: f64,
    /// Lower bound, clamped at 0.
    pub lo: f64,
    /// Upper bound.
    pub hi: f64,
}

impl MeanCi {
    /// Half-width before clamping.
    #[must_use]
    pub fn half_width(&self) -> f64 {
        self.hi - self.mean
    }
}

/// Student t 95% interval on the mean of `values` (n - 1 degrees of freedom),
/// with the lower bound clamped at 0. One value gives an unbounded interval;
/// no values give `None`.
#[must_use]
pub fn t_interval(values: &[f64]) -> Option<MeanCi> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    let mean = values.iter().sum::<f64>() / n as f64;
    if n == 1 {
        return Some(MeanCi {
            n,
            mean,
            lo: 0.0,
            hi: f64::INFINITY,
        });
    }
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    let half = t975(n - 1) * (var / n as f64).sqrt();
    Some(MeanCi {
        n,
        mean,
        lo: (mean - half).max(0.0),
        hi: mean + half,
    })
}

/// Completed trials out of `n`, with the Wilson 95% interval on the
/// completion proportion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Completion {
    /// Trials that delivered every byte.
    pub completed: usize,
    /// Trials.
    pub n: usize,
    /// Wilson lower bound.
    pub lo: f64,
    /// Wilson upper bound.
    pub hi: f64,
}

/// Completion proportion with its Wilson interval.
#[must_use]
pub fn completion(completed: usize, n: usize) -> Completion {
    let (lo, hi) = wilson_ci95(completed, n);
    Completion {
        completed,
        n,
        lo,
        hi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_interval_matches_textbook() {
        let ci = t_interval(&[1.0, 2.0, 3.0, 4.0, 5.0]).unwrap();
        assert_eq!(ci.n, 5);
        assert!((ci.mean - 3.0).abs() < 1e-12);
        // s = sqrt(2.5), t(4) = 2.776: 2.776 * sqrt(2.5 / 5) = 1.963.
        assert!((ci.half_width() - 1.963).abs() < 1e-3, "{ci:?}");
        assert!((ci.lo - 1.037).abs() < 1e-3);
    }

    #[test]
    fn t_quantiles_match_tables() {
        for (df, t) in [
            (1, 12.706),
            (7, 2.365),
            (30, 2.042),
            (120, 1.980),
            (1000, 1.962),
        ] {
            assert!((t975(df) - t).abs() < 1e-3, "df {df}: {}", t975(df));
        }
        // Monotone decreasing toward the normal limit.
        let mut prev = t975(1);
        for df in 2..2000 {
            let t = t975(df);
            assert!(t <= prev + 1e-12, "df {df}");
            assert!(t > Z975);
            prev = t;
        }
        assert!(t975(0).is_infinite());
    }

    #[test]
    fn equal_values_give_zero_width() {
        let ci = t_interval(&[1495.0; 8]).unwrap();
        assert_eq!(ci.mean, 1495.0);
        assert_eq!(ci.lo, 1495.0);
        assert_eq!(ci.hi, 1495.0);
        assert!(t_interval(&[]).is_none());
    }

    #[test]
    fn lower_bound_clamps_at_zero() {
        let ci = t_interval(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 800.0]).unwrap();
        assert_eq!(ci.lo, 0.0);
        assert!(ci.mean > 0.0 && ci.hi > ci.mean);
    }

    #[test]
    fn completion_uses_wilson() {
        let c = completion(7, 8);
        let (lo, hi) = wilson_ci95(7, 8);
        assert_eq!((c.lo, c.hi), (lo, hi));
        assert_eq!((c.completed, c.n), (7, 8));
        assert!((c.lo - 0.53).abs() < 0.01 && (c.hi - 0.98).abs() < 0.01);
    }
}
