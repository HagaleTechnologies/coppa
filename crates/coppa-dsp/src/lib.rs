//! Pure DSP primitives for the Coppa digital communications system.
//!
//! Provides filtering, AGC, carrier recovery, timing recovery, and FFT utilities.

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
