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
//! - `g3ruh`: the 9600 baud direct-FSK modem for the KISS TNC.
//!
//! # Example
//!
//! One OFDM frame with BPSK payload symbols, modulated by
//! [`ofdm::coppa_modem::CoppaModem`] and demodulated again. No FEC is applied
//! here; `coppa_protocol::modem::CoppaTransceiver` adds LDPC and interleaving
//! on top of this layer. The demodulator can return a few symbols fewer than
//! were sent, so the example compares a prefix rather than the full length.
//!
//! ```
//! use coppa_codec::bpsk::BpskMapper;
//! use coppa_codec::ofdm::coppa_modem::{CoppaModem, SPEED_LEVELS};
//! use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
//! use coppa_codec::ofdm::CoppaProfile;
//! use coppa_codec::ConstellationMapper;
//!
//! let modem = CoppaModem::new(CoppaProfile::hf_standard(), 1);
//! // One LDPC codeword's worth of coded bits (1,944) as BPSK symbols.
//! let bits: Vec<u8> = (0..1944).map(|i| ((i * 7 + 3) % 5 % 2) as u8).collect();
//! let symbols = BpskMapper.map_bits(&bits);
//! let header = CoppaHeader {
//!     version: 1,
//!     phy_mode: 0,
//!     frame_type: CoppaFrameType::Data,
//!     bandwidth: CoppaProfile::hf_standard().bandwidth_id,
//!     fec_type: 0,
//!     speed_level: 1,
//!     seq_num: 7,
//!     payload_len: 0,
//!     codewords: 1,
//! };
//! let audio = modem.modulate_mapped(&header, &symbols, SPEED_LEVELS[0].papr_target_db);
//!
//! let (rx_header, rx_symbols, _noise, _snr) =
//!     modem.demodulate_frame(&audio).expect("clean loopback finds the frame");
//! assert_eq!(rx_header.seq_num, 7);
//! assert_eq!(rx_header.speed_level, 1);
//! let rx_bits: Vec<u8> = rx_symbols[..64]
//!     .iter()
//!     .flat_map(|&s| BpskMapper.demap_hard(s))
//!     .collect();
//! assert_eq!(rx_bits, bits[..64]);
//! ```

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

#![warn(missing_docs, missing_debug_implementations)]

pub mod error;
pub use error::CodecError;

pub mod afsk;
pub mod bpsk;
mod fx25;
pub mod g3ruh;
mod hdlc;
pub mod il2p;
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
