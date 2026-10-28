//! Core engine for the Coppa digital communications system.
//!
//! This crate provides:
//! - [`CoppaCore`] - the main encode/decode engine integrating modem, FEC, and framing
//! - [`EngineConfig`] - runtime configuration types
//! - [`profiles`] - the named operating profiles (`HF_ROBUST`, `HF_STANDARD`,
//!   `VHF_FAST`, `EMERGENCY`) that `EngineConfig::from_profile` builds from

pub mod config;
pub mod engine;

pub mod profiles;

pub use config::{EngineConfig, OfdmBand, HF_MAX_SPEED_LEVEL};
pub use engine::{CoppaCore, StreamFrame, TUNE_TONE_HIGH_HZ, TUNE_TONE_LOW_HZ};
pub use profiles::{Profile, EMERGENCY, HF_ROBUST, HF_STANDARD, VHF_FAST};
