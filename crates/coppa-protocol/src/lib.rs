//! Protocol layer for Coppa.
//!
//! Provides:
//! - `modem`: `CoppaTransceiver`, the encode/decode pipeline that composes the
//!   OFDM modem, LDPC coding, constellation mapping and interleaving, plus
//!   `StreamingReceiver`, the speed-level tables and the airtime model,
//! - `fec`: the NR BG2 LDPC code every payload uses and the scrambler, plus
//!   legacy 802.11 QC-LDPC codes and a K=7 convolutional code that no
//!   transmit or receive path uses,
//! - PHY framing, MAC/transport/application PDUs, selective-repeat ARQ,
//!   sessions, compression, the AX.25 UI frame codec, and the CP-profile
//!   negotiator (`cp_negotiator`).
//!
//! # Example
//!
//! One frame through [`modem::CoppaTransceiver`] and back, with no channel in
//! between. The transceiver LDPC-encodes, interleaves and maps the payload,
//! then modulates it into 48 kHz audio; `receive` reverses every step.
//!
//! ```
//! use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
//! use coppa_codec::ofdm::CoppaProfile;
//! use coppa_protocol::modem::CoppaTransceiver;
//!
//! let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
//! let payload = b"CQ CQ DE N0CALL";
//! let header = CoppaHeader {
//!     version: 1,
//!     phy_mode: 0,
//!     frame_type: CoppaFrameType::Data,
//!     bandwidth: transceiver.profile().bandwidth_id,
//!     fec_type: 0,
//!     speed_level: 1,
//!     seq_num: 0,
//!     payload_len: payload.len() as u16,
//!     codewords: 1,
//! };
//! let audio = transceiver.transmit(&header, payload).expect("payload fits level 1");
//! let (rx_header, rx_payload, _recommended_level) =
//!     transceiver.receive(&audio).expect("clean loopback decodes");
//! assert_eq!(rx_header.speed_level, 1);
//! assert_eq!(rx_payload, payload);
//! ```

//! # Errors
//!
//! Protocol validation methods now return [`ProtocolError`] instead of an
//! opaque error. This is a source-breaking signature change. Match its typed
//! fields and keep a wildcard arm because the enum is non-exhaustive.
//! Existing transceiver `TransmitError` and `ReceiveError` remain separate.
//! `TransmitError` is also `#[non_exhaustive]`; `ReceiveError` deliberately
//! is not, because coppa-bench buckets every variant and a wildcard arm there
//! would hide a new one.
//!
//! ```
//! use coppa_protocol::{mac::Callsign, ProtocolError};
//! match Callsign::new("BAD!CHAR") {
//!     Err(ProtocolError::InvalidCallsignCharacter { character }) => assert_eq!(character, '!'),
//!     other => panic!("unexpected result: {other:?}"),
//! }
//! ```
//!
//! # Portability
//!
//! The crate compiles for `wasm32-unknown-unknown` and never reads the clock:
//! every method that needs the time takes `now: std::time::Instant` from the
//! caller. That still leaves `Session`, `ArqTx` and `CpNegotiator` unusable on
//! `wasm32-unknown-unknown`, because a caller there cannot construct an
//! `Instant` (`Instant::now()` traps), until a crate-wide timestamp type exists
//! (see `docs/LIMITATIONS.md`).

#![warn(missing_docs, missing_debug_implementations)]
// Unit tests build synthetic `Instant`s from `Instant::now()`; the
// clippy.toml clock ban (COP-90) applies to library code only.
#![cfg_attr(test, allow(clippy::disallowed_methods))]

pub mod error;
pub use error::ProtocolError;

pub mod app;
pub mod arq;
pub mod broadcast_file;
pub mod cp_negotiator;
pub mod fec;
pub mod frame;
pub mod mac;
pub mod outer_code;
pub mod session;
pub mod transport;

pub mod compression;

pub mod ax25;

/// The modem pipeline: `CoppaTransceiver`, `StreamingReceiver`, the speed
/// levels and the airtime model.
pub mod modem;

pub use frame::Frame;
