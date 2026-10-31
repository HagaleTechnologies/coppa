//! Modulation and demodulation codecs for Coppa.
//!
//! Provides:
//! - `ofdm`: the OFDM PHY every speed level runs on (`CoppaModem`,
//!   `CoppaProfile`, the streaming `SyncDetector`), plus a simpler generic
//!   OFDM stack kept as a teaching example,
//! - constellation mappers for BPSK, QPSK, 8PSK, 16QAM and 64QAM behind the
//!   `ConstellationMapper` trait, and the `Modem`, `FecCodec` and
//!   `ChannelEstimator` traits,
//! - `afsk`: the AFSK 1200 (Bell 202) modem with HDLC framing and NRZI coding
//!   that the KISS TNC uses.

//! # Errors
//!
//! Fallible modem methods now return [`CodecError`] instead of an opaque error.
//! This is a source-breaking signature change for trait implementors. The enum
//! is empty because BPSK currently has no failure conditions; empty and short
//! inputs remain supported. It is non-exhaustive, so callers outside this crate
//! must allow for future variants.
//!
//! ```
//! use coppa_codec::{BpskModem, Modem, CodecError};
//! fn audio() -> Result<Vec<f32>, CodecError> {
//!     BpskModem::new().modulate(&[0, 1])
//! }
//! match audio() {
//!     Ok(samples) => assert!(!samples.is_empty()),
//!     Err(error) => eprintln!("{error}"),
//! }
//! ```

pub mod error;
pub use error::CodecError;

pub mod afsk;
pub mod bpsk;
pub mod ofdm;
pub mod psk8;
pub mod qam16;
pub mod qam64;
pub mod qpsk;
pub mod traits;

#[cfg(test)]
mod qam_oracle_tol;

pub use bpsk::BpskModem;
pub use traits::{ConstellationMapper, Modem};
