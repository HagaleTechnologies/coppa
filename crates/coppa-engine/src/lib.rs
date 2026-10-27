//! Core engine for the Coppa digital communications system.
//!
//! This crate provides:
//! - [`CoppaCore`] - the main encode/decode engine integrating modem, FEC, and framing
//! - [`EngineConfig`] - runtime configuration types
//! - [`ladder`] - the real per-level facts (modulation, code rate, max bytes,
//!   air time, bandwidth) that `coppa profiles` shows

pub mod config;
pub mod engine;
pub mod ladder;

pub mod profiles;

pub use config::{EngineConfig, OfdmBand, HF_MAX_SPEED_LEVEL};
pub use engine::{CoppaCore, StreamFrame, TUNE_TONE_HIGH_HZ, TUNE_TONE_LOW_HZ};
pub use profiles::{Profile, EMERGENCY, HF_ROBUST, HF_STANDARD, VHF_FAST};
