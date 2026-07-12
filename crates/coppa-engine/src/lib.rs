//! Core engine for the Coppa digital communications system.
//!
//! This crate provides:
//! - [`CoppaCore`] - the main encode/decode engine integrating modem, FEC, and framing
//! - [`EngineConfig`] - runtime configuration types

pub mod config;
pub mod engine;

pub mod profiles;

pub use config::EngineConfig;
pub use engine::{CoppaCore, TUNE_TONE_HIGH_HZ, TUNE_TONE_LOW_HZ};
pub use profiles::{Profile, EMERGENCY, HF_ROBUST, HF_STANDARD, VHF_FAST};
