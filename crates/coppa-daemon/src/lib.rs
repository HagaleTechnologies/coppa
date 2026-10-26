//! Coppa daemon library — exposes TNC and daemon components for use by other crates.

pub mod audio_output;
pub mod config;
pub mod event_loop;
pub mod ptt_guardian;
#[cfg(feature = "websocket")]
pub mod spectrum;
#[cfg(feature = "kiss-tnc")]
pub mod tnc;
