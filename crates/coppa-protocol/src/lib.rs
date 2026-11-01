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

//! # Errors
//!
//! Protocol validation methods now return [`ProtocolError`] instead of an
//! opaque error. This is a source-breaking signature change. Match its typed
//! fields and keep a wildcard arm because the enum is non-exhaustive.
//! Existing transceiver `TransmitError` and `ReceiveError` remain separate.
//!
//! ```
//! use coppa_protocol::{mac::Callsign, ProtocolError};
//! match Callsign::new("BAD!CHAR") {
//!     Err(ProtocolError::InvalidCallsignCharacter { character }) => assert_eq!(character, '!'),
//!     other => panic!("unexpected result: {other:?}"),
//! }
//! ```

pub mod error;
pub use error::ProtocolError;

pub mod app;
pub mod arq;
pub mod cp_negotiator;
pub mod fec;
pub mod frame;
pub mod mac;
pub mod session;
pub mod transport;

pub mod compression;

pub mod ax25;

pub mod modem;

pub use frame::Frame;
