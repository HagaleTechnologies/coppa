//! Coppa daemon library — exposes TNC and daemon components for use by other crates.

pub mod audio_output;
pub mod build_info;
pub mod config;
pub mod event_loop;
pub mod level_meter;
pub mod ptt_guardian;
#[cfg(feature = "websocket")]
pub mod spectrum;
pub mod startup_report;
#[cfg(feature = "kiss-tnc")]
pub mod tnc;
pub mod tx_chunker;
