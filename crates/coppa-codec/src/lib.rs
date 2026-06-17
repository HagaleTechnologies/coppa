//! Modulation and demodulation codecs for Coppa.
//!
//! Provides the `ConstellationMapper` trait for constellation mapping/demapping,
//! the `Modem` trait for complete modulation/demodulation, and implementations
//! for BPSK, QPSK, 8PSK, 16QAM, and 64QAM.

pub mod afsk;
pub mod bpsk;
pub mod ofdm;
pub mod psk8;
pub mod qam16;
pub mod qam64;
pub mod qpsk;
pub mod traits;

pub use bpsk::BpskModem;
pub use ofdm::modem::OfdmModem;
pub use traits::{ConstellationMapper, Modem};
