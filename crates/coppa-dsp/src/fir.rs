//! Linear-phase FIR design (windowed sinc) and block filtering.

/// Odd-length linear-phase bandpass via Blackman-windowed sinc.
pub fn design_bandpass(taps: usize, fs: f32, f_lo: f32, f_hi: f32) -> Vec<f32> {
    assert!(taps % 2 == 1, "linear phase needs odd length");
    let m = (taps - 1) as f32 / 2.0;
    let (w_lo, w_hi) = (
        std::f32::consts::TAU * f_lo / fs,
        std::f32::consts::TAU * f_hi / fs,
    );
    (0..taps)
        .map(|i| {
            let k = i as f32 - m;
            let ideal = if k.abs() < 1e-9 {
                (w_hi - w_lo) / std::f32::consts::PI
            } else {
                ((w_hi * k).sin() - (w_lo * k).sin()) / (std::f32::consts::PI * k)
            };
            let x = std::f32::consts::TAU * i as f32 / (taps - 1) as f32;
            let win = 0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos();
            ideal * win
        })
        .collect()
}

/// Odd-length Hilbert transformer (type-III FIR): h[k] = 2/(pi*k) for odd k, else 0,
/// Blackman-windowed. Pass real x; get the quadrature component with (taps-1)/2 delay.
pub fn design_hilbert(taps: usize) -> Vec<f32> {
    assert!(taps % 2 == 1, "Hilbert FIR needs odd length");
    let m = (taps - 1) as i64 / 2;
    (0..taps)
        .map(|i| {
            let k = i as i64 - m;
            let ideal = if k != 0 && k % 2 != 0 {
                2.0 / (std::f32::consts::PI * k as f32)
            } else {
                0.0
            };
            let x = std::f32::consts::TAU * i as f32 / (taps - 1) as f32;
            let win = 0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos();
            ideal * win
        })
        .collect()
}

/// Streaming FIR with carry-over state (history of `taps - 1` input samples), for
/// filtering a signal delivered in chunks without discontinuities at chunk boundaries.
pub struct StreamingFir {
    coeffs: Vec<f32>,
    /// The last `coeffs.len() - 1` input samples seen, oldest first.
    history: Vec<f32>,
    /// `(k, coeffs[k])` for every structurally-nonzero tap. Designs like
    /// `design_hilbert` are exactly half zero by construction (even lags); skipping
    /// those multiplications is a pure speedup (identical output, since a zero
    /// coefficient contributes nothing) and matters on the per-sample hot path
    /// (e.g. `SyncDetector`'s streaming analytic signal).
    nonzero: Vec<(usize, f32)>,
}

impl StreamingFir {
    pub fn new(coeffs: Vec<f32>) -> Self {
        let hist_len = coeffs.len().saturating_sub(1);
        let nonzero = coeffs
            .iter()
            .enumerate()
            .filter(|&(_, &c)| c != 0.0)
            .map(|(k, &c)| (k, c))
            .collect();
        Self {
            coeffs,
            history: vec![0.0; hist_len],
            nonzero,
        }
    }

    /// Filter `x`, appending exactly `x.len()` output samples to `out`. Carries
    /// convolution state across calls, so pushing a signal in arbitrary-sized
    /// chunks produces the same output as filtering it all at once.
    pub fn process(&mut self, x: &[f32], out: &mut Vec<f32>) {
        let taps = self.coeffs.len();
        if taps == 0 || x.is_empty() {
            return;
        }
        let hist_len = self.history.len();
        let mut buf = Vec::with_capacity(hist_len + x.len());
        buf.extend_from_slice(&self.history);
        buf.extend_from_slice(x);

        out.reserve(x.len());
        for n in 0..x.len() {
            let window = &buf[n..n + taps];
            let mut acc = 0.0f32;
            for &(k, c) in &self.nonzero {
                acc += c * window[taps - 1 - k];
            }
            out.push(acc);
        }

        if hist_len > 0 {
            let total = buf.len();
            self.history.copy_from_slice(&buf[total - hist_len..]);
        }
    }
}

pub struct Fir {
    coeffs: Vec<f32>,
}

impl Fir {
    pub fn new(coeffs: Vec<f32>) -> Self {
        Self { coeffs }
    }
    /// Group delay in samples: (taps-1)/2.
    pub fn group_delay(&self) -> usize {
        (self.coeffs.len() - 1) / 2
    }
    /// Convolve a block (output length = input length; zero-padded edges).
    pub fn filter_block(&self, x: &[f32]) -> Vec<f32> {
        let l = self.coeffs.len();
        let mut y = vec![0.0f32; x.len()];
        for (i, out) in y.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for (j, &c) in self.coeffs.iter().enumerate() {
                if i + j >= l - 1 {
                    let k = i + j - (l - 1);
                    if k < x.len() {
                        acc += c * x[k];
                    }
                }
            }
            *out = acc;
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bandpass_rejects_out_of_band_tones() {
        let h = design_bandpass(601, 48_000.0, 250.0, 2850.0);
        let fir = Fir::new(h);
        let tone = |f: f32| -> f32 {
            // steady-state gain at frequency f: filter 4800 samples, measure RMS of the tail
            let x: Vec<f32> = (0..4800)
                .map(|i| (std::f32::consts::TAU * f * i as f32 / 48_000.0).sin())
                .collect();
            let y = fir.filter_block(&x);
            let tail = &y[2400..];
            (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt()
        };
        let in_band = tone(1500.0);
        assert!((tone(100.0) / in_band) < 0.03, "-30 dB at 100 Hz");
        assert!((tone(4000.0) / in_band) < 0.03, "-30 dB at 4 kHz");
        assert!(
            (0.9..1.1).contains(&(tone(500.0) / in_band)),
            "flat passband"
        );
    }

    #[test]
    #[should_panic(expected = "odd length")]
    fn design_hilbert_rejects_even_taps() {
        design_hilbert(128);
    }

    #[test]
    fn design_hilbert_is_antisymmetric_with_zeroed_center_and_even_taps() {
        let taps = 129;
        let h = design_hilbert(taps);
        assert_eq!(h.len(), taps);
        let m = (taps - 1) / 2;
        assert_eq!(h[m], 0.0, "center tap (k=0) must be exactly zero");
        for k in 1..=m {
            // Antisymmetric about the center: h[m-k] == -h[m+k].
            assert!(
                (h[m - k] + h[m + k]).abs() < 1e-6,
                "taps at +-{k} should be antisymmetric, got {} and {}",
                h[m - k],
                h[m + k]
            );
        }
        // Even-offset (k even, k!=0) taps must be exactly zero per the design formula.
        for k in (2..=m).step_by(2) {
            assert_eq!(h[m + k], 0.0, "even-lag tap {k} should be zero");
        }
    }

    #[test]
    fn hilbert_fir_produces_quadrature_with_expected_group_delay() {
        // A steady sine at frequency f, delayed by the filter's group delay,
        // should be 90 degrees out of phase with its Hilbert transform: i.e.
        // causally convolving x(n) = sin(wn) with the Hilbert FIR should produce
        // +-cos(w(n - group_delay)) in steady state (a quarter-cycle shift). The
        // absolute sign is a convention (irrelevant to the Schmidl-Cox |P|^2
        // metric downstream, which is invariant to a global sign/conjugate flip
        // of the analytic signal), so this only checks the magnitude/phase
        // relationship, not a specific sign.
        let taps = 129;
        let group_delay = (taps - 1) / 2;
        let h = design_hilbert(taps);
        let mut fir = StreamingFir::new(h);
        let fs = 48_000.0f32;
        let f = 1500.0f32;
        let n = 4800;
        let x: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * f * i as f32 / fs).sin())
            .collect();
        let mut y = Vec::new();
        fir.process(&x, &mut y);

        // Compare y[i] against +-cos(w*(i - group_delay)) over a steady-state tail.
        let mut err_pos = 0.0f32;
        let mut err_neg = 0.0f32;
        let mut count = 0;
        for (i, &yi) in y.iter().enumerate().take(n).skip(n / 2) {
            let expected = (std::f32::consts::TAU * f * (i as f32 - group_delay as f32) / fs).cos();
            err_pos += (yi - expected).powi(2);
            err_neg += (yi + expected).powi(2);
            count += 1;
        }
        let rmse = (err_pos.min(err_neg) / count as f32).sqrt();
        assert!(
            rmse < 0.05,
            "Hilbert output should track +-cos(w(n-delay)), rmse={rmse}"
        );
    }

    #[test]
    fn streaming_fir_matches_single_shot_across_chunk_boundaries() {
        // The streaming FIR must produce identical output whether fed all at once
        // or in arbitrary-sized chunks (carry-over state must seam chunks exactly).
        let h = design_hilbert(129);
        let n = 2000;
        let x: Vec<f32> = (0..n)
            .map(|i| (i as f32 * 0.05).sin() + 0.3 * (i as f32 * 0.011).cos())
            .collect();

        let mut single_shot = StreamingFir::new(h.clone());
        let mut expected = Vec::new();
        single_shot.process(&x, &mut expected);

        let mut streaming = StreamingFir::new(h);
        let mut got = Vec::new();
        for chunk in x.chunks(37) {
            streaming.process(chunk, &mut got);
        }
        assert_eq!(got.len(), expected.len());
        for (i, (&a, &b)) in got.iter().zip(expected.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-4,
                "streaming vs single-shot mismatch at {i}: {a} vs {b}"
            );
        }
    }
}
