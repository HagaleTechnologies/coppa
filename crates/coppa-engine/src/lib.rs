//! Core engine for the Coppa digital communications system.
//!
//! This crate provides:
//! - [`CoppaCore`] - the main encode/decode engine integrating modem, FEC, and framing
//! - [`EngineConfig`] - runtime configuration types
//! - [`profiles`] - the named operating profiles (`HF_ROBUST`, `HF_STANDARD`,
//!   `VHF_FAST`, `EMERGENCY`) that `EngineConfig::from_profile` builds from
//! - [`ladder`] - the real per-level facts (modulation, code rate, max bytes,
//!   air time, bandwidth) that `coppa profiles` shows
//! - [`morse`] - Morse (CW) keying for the station's closing CW ID
//! - [`link`] - a connected, reliable session over audio samples (coppad's
//!   session protocol, without the daemon)
//!
//! # Errors
//!
//! Fallible methods and streaming payloads now return [`EngineError`] in place
//! of an opaque error. This is a source-breaking signature change. Match the
//! failure kinds you handle and keep a wildcard for future variants.
//!
//! ```
//! use coppa_engine::{CoppaCore, EngineError};
//! use coppa_protocol::modem::TransmitError;
//! let core = CoppaCore::new();
//! match core.encode_bytes_with_level(&[0xAB; 4096], 1) {
//!     Err(EngineError::Transmit(TransmitError::PayloadTooLarge { max })) => assert_eq!(max, 56),
//!     other => panic!("unexpected result: {other:?}"),
//! }
//! ```

#![warn(missing_docs, missing_debug_implementations)]
// COP-139: the crate takes `now: Instant` from its caller (clippy.toml); its
// own tests may read the clock to anchor virtual time.
#![cfg_attr(test, allow(clippy::disallowed_methods))]

pub mod config;
pub mod engine;
pub mod ladder;
pub mod link;
pub mod morse;

pub mod profiles;

pub use config::{EngineConfig, OfdmBand, HF_MAX_SPEED_LEVEL};
pub use engine::{
    CoppaCore, StreamFrame, MAX_TUNE_SECONDS, TUNE_SINGLE_MAX_HZ, TUNE_SINGLE_MIN_HZ,
    TUNE_TONE_HIGH_HZ, TUNE_TONE_LOW_HZ, WIRE_FORMAT_VERSION,
};
pub use link::{DisconnectReason, Link, LinkConfig, LinkError, LinkEvent, LinkState};
pub use profiles::{Profile, EMERGENCY, HF_ROBUST, HF_STANDARD, VHF_FAST};

pub mod error;
pub use error::EngineError;
