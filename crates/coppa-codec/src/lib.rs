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
