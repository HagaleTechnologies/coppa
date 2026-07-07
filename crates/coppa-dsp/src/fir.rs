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
}
