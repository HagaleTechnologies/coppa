//! Pure DSP primitives for the Coppa digital communications system.
//!
//! Provides:
//! - `fft`: forward and inverse FFTs,
//! - `fir` and `filter`: windowed-sinc bandpass, Hilbert and root-raised-cosine
//!   FIR design, plus block and streaming filters,
//! - `agc`: automatic gain control,
//! - `carrier_recovery`: a Costas loop,
//! - `timing_recovery`: a Gardner symbol-timing loop.
//!
//! No other coppa crate is a dependency.
//!
//! # Example
//!
//! Band-limit 48 kHz audio to an SSB voice passband (300-2,700 Hz) with a
//! 201-tap bandpass. A 1 kHz tone passes; a 10 kHz tone is rejected.
//!
//! ```
//! use coppa_dsp::fir::{design_bandpass, Fir};
//!
//! let fs = 48_000.0;
//! let bandpass = Fir::new(design_bandpass(201, fs, 300.0, 2_700.0));
//! let tone = |hz: f32| -> Vec<f32> {
//!     (0..4_800)
//!         .map(|n| (std::f32::consts::TAU * hz * n as f32 / fs).sin())
//!         .collect()
//! };
//! // RMS after the filter's start-up transient (its group delay).
//! let rms = |x: &[f32]| -> f32 {
//!     let tail = &x[bandpass.group_delay() * 2..];
//!     (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt()
//! };
//!
//! let in_band = bandpass.filter_block(&tone(1_000.0));
//! let out_of_band = bandpass.filter_block(&tone(10_000.0));
//! assert!(rms(&in_band) > 0.6, "1 kHz passes (input RMS is 0.707)");
//! assert!(rms(&out_of_band) < 0.01, "10 kHz is rejected");
//! ```

#![warn(missing_docs, missing_debug_implementations)]
// COP-90 (H-134): this is the crate skimmer consumes; a pure function whose
// result is dropped is always a bug, so new ones must carry `#[must_use]`.
#![warn(clippy::must_use_candidate)]

/// The complex sample type used throughout this crate's API, re-exported so a
/// caller does not need its own matching `num-complex` dependency.
pub use num_complex::{Complex, Complex32};

/// Block-adaptive automatic gain control.
pub mod agc;
pub mod carrier_recovery;
pub mod fft;
pub mod filter;
pub mod fir;
/// Gardner symbol timing recovery and cubic interpolation.
pub mod timing_recovery;
