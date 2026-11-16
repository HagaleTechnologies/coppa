//! A required signal-path test before a black-box measurement.

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub correlation: f64,
    pub level_db: f64,
    pub cable_latency_ms: f64,
}

pub fn analyse(
    sent: &[f32],
    received: &[f32],
    sample_rate: usize,
    starved: u64,
    overflow: u64,
) -> Result<Metrics> {
    if sample_rate == 0 || sent.len() < sample_rate || received.len() < sent.len() {
        bail!("integrity probe requires the whole transmitted signal to be captured");
    }
    // Find the lead by normalized cross-correlation over a bounded 1 s window.
    // A broadband, nonperiodic probe makes the maximum unambiguous.
    let limit = received.len().saturating_sub(sent.len()).min(sample_rate);
    let n = sample_rate;
    let mut best = (f64::NEG_INFINITY, 0usize, 0.0f64);
    let sx = sent[..n]
        .iter()
        .step_by(24)
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>();
    if sx <= 0.0 {
        bail!("integrity probe sent silence");
    }
    let score = |lag: usize| {
        let mut dot = 0.0;
        let mut sy = 0.0;
        for (&x, &y) in sent[..n]
            .iter()
            .step_by(24)
            .zip(received[lag..lag + n].iter().step_by(24))
        {
            if !y.is_finite() {
                bail!("integrity probe received non-finite audio");
            }
            dot += f64::from(x) * f64::from(y);
            sy += f64::from(y).powi(2);
        }
        let corr = if sy > 0.0 {
            dot / (sx * sy).sqrt()
        } else {
            0.0
        };
        Ok::<_, anyhow::Error>((corr, sy))
    };
    for lag in (0..=limit).step_by(16) {
        let (corr, sy) = score(lag)?;
        if corr > best.0 {
            best = (corr, lag, sy);
        }
    }
    for lag in best.1.saturating_sub(15)..=(best.1 + 15).min(limit) {
        let (corr, sy) = score(lag)?;
        if corr > best.0 {
            best = (corr, lag, sy);
        }
    }
    let full_sent = sent.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
    let mut full_dot = 0.0;
    let mut full_received = 0.0;
    for (&x, &y) in sent.iter().zip(&received[best.1..best.1 + sent.len()]) {
        if !y.is_finite() {
            bail!("integrity probe received non-finite audio");
        }
        full_dot += f64::from(x) * f64::from(y);
        full_received += f64::from(y).powi(2);
    }
    let full_correlation = full_dot / (full_sent * full_received).sqrt();
    let level_db = 10.0 * (full_received / full_sent).log10();
    let metrics = Metrics {
        correlation: full_correlation,
        level_db,
        cable_latency_ms: best.1 as f64 * 1000.0 / sample_rate as f64,
    };
    if metrics.correlation < 0.999 || metrics.level_db.abs() > 0.1 || starved != 0 || overflow != 0
    {
        bail!("integrity probe FAIL: correlation {:.6}, level {:+.2} dB, {starved} starved, {overflow} overflow", metrics.correlation, metrics.level_db);
    }
    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn signal() -> Vec<f32> {
        (0..1000)
            .map(|i| ((i * 7919 % 997) as f32 / 997.0 - 0.5) * 0.2)
            .collect()
    }
    #[test]
    fn identical_signals_pass() {
        let s = signal();
        assert!(analyse(&s, &s, 1000, 0, 0).is_ok());
    }
    #[test]
    fn a_dropped_block_fails() {
        let s = signal();
        let mut r = s.clone();
        r[300..400].fill(0.0);
        assert!(analyse(&s, &r, 1000, 0, 0).is_err());
    }
    #[test]
    fn a_gain_error_fails() {
        let s = signal();
        let r: Vec<_> = s.iter().map(|v| v * 0.75).collect();
        assert!(analyse(&s, &r, 1000, 0, 0).is_err());
    }
    #[test]
    fn zero_padded_capture_fails() {
        let s = signal();
        let mut r = s.clone();
        for v in r.iter_mut().step_by(3) {
            *v = 0.0;
        }
        assert!(analyse(&s, &r, 1000, 0, 0).is_err());
    }
}
