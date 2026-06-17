//! VARA-style TCP control protocol (not RF/waveform-compatible with VARA).
//!
//! Provides command port (8300) and data port (8301) modeled on the VARA HF
//! TCP control protocol. The wire-level control protocol is VARA-style, but the
//! Coppa modem is NOT RF/waveform-compatible with VARA.

pub mod command;
pub mod data;
pub mod protocol;
pub mod server;

pub use protocol::{VaraCommand, VaraResponse};
pub use server::VaraServer;
