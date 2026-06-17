//! FFT utilities for OFDM processing.
//!
//! Wraps `rustfft` to provide convenient forward/inverse FFT operations
//! for OFDM modulation and demodulation. The planner and FFT objects are
//! cached at construction time for efficient repeated use.
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

#[derive(Debug)]
pub enum FftError {
    ZeroSize,
    LengthMismatch { expected: usize, got: usize },
}

impl std::fmt::Display for FftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FftError::ZeroSize => write!(f, "FFT size must be > 0"),
            FftError::LengthMismatch { expected, got } => {
                write!(
                    f,
                    "FFT input length {} != configured size {}",
                    got, expected
                )
            }
        }
    }
}

impl std::error::Error for FftError {}

/// A cached forward/inverse FFT pair of a fixed size.
///
/// The inverse transform is scaled by `1/N`, so a forward followed by an
/// inverse reconstructs the original signal:
///
/// ```
/// use coppa_dsp::fft::FftProcessor;
/// use num_complex::Complex32;
///
/// let fft = FftProcessor::new(8);
/// let input: Vec<Complex32> = (0..8)
///     .map(|i| Complex32::new(i as f32, 0.0))
///     .collect();
///
/// let freq = fft.forward(&input);
/// let reconstructed = fft.inverse(&freq);
///
/// for (a, b) in input.iter().zip(reconstructed.iter()) {
///     assert!((a - b).norm() < 1e-4);
/// }
/// ```
pub struct FftProcessor {
    size: usize,
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
}

impl FftProcessor {
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "FFT size must be > 0");
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(size);
        let inv = planner.plan_fft_inverse(size);
        Self { size, fwd, inv }
    }

    /// Try to create a new FFT processor, returning an error if size is 0.
    pub fn try_new(size: usize) -> Result<Self, FftError> {
        if size == 0 {
            return Err(FftError::ZeroSize);
        }
        Ok(Self::new(size))
    }

    /// Forward FFT: time domain -> frequency domain.
    pub fn forward(&self, input: &[Complex32]) -> Vec<Complex32> {
        assert_eq!(
            input.len(),
            self.size,
            "FFT input length {} != configured size {}",
            input.len(),
            self.size
        );
        let mut buffer = vec![Complex32::new(0.0, 0.0); self.size];
        buffer[..self.size].copy_from_slice(&input[..self.size]);

        self.fwd.process(&mut buffer);
        buffer
    }

    /// Forward FFT with error handling instead of panic.
    pub fn try_forward(&self, input: &[Complex32]) -> Result<Vec<Complex32>, FftError> {
        if input.len() != self.size {
            return Err(FftError::LengthMismatch {
                expected: self.size,
                got: input.len(),
            });
        }
        Ok(self.forward(input))
    }

    /// Inverse FFT: frequency domain -> time domain.
    /// Output is scaled by 1/N.
    pub fn inverse(&self, input: &[Complex32]) -> Vec<Complex32> {
        assert_eq!(
            input.len(),
            self.size,
            "IFFT input length {} != configured size {}",
            input.len(),
            self.size
        );
        let mut buffer = vec![Complex32::new(0.0, 0.0); self.size];
        buffer[..self.size].copy_from_slice(&input[..self.size]);

        self.inv.process(&mut buffer);

        let scale = 1.0 / self.size as f32;
        for s in &mut buffer {
            *s *= scale;
        }
        buffer
    }

    /// Inverse FFT with error handling instead of panic.
    pub fn try_inverse(&self, input: &[Complex32]) -> Result<Vec<Complex32>, FftError> {
        if input.len() != self.size {
            return Err(FftError::LengthMismatch {
                expected: self.size,
                got: input.len(),
            });
        }
        Ok(self.inverse(input))
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fft_roundtrip() {
        let fft = FftProcessor::new(256);
        let input: Vec<Complex32> = (0..256)
            .map(|i| Complex32::new((i as f32 * 0.1).sin(), 0.0))
            .collect();

        let freq = fft.forward(&input);
        let reconstructed = fft.inverse(&freq);

        for (a, b) in input.iter().zip(reconstructed.iter()) {
            assert!(
                (a - b).norm() < 1e-4,
                "FFT roundtrip should preserve signal"
            );
        }
    }

    #[test]
    fn test_fft_dc_bin() {
        let fft = FftProcessor::new(64);
        let input = vec![Complex32::new(1.0, 0.0); 64];
        let freq = fft.forward(&input);
        assert!(
            (freq[0].re - 64.0).abs() < 1e-4,
            "DC bin should be N for constant input"
        );
        for bin in freq.iter().skip(1) {
            assert!(
                bin.norm() < 1e-4,
                "Non-DC bins should be zero for constant input"
            );
        }
    }

    #[test]
    #[should_panic(expected = "FFT size must be > 0")]
    fn test_fft_zero_size_panics() {
        FftProcessor::new(0);
    }
}
