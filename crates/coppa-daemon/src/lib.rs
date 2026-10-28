//! Coppa daemon library — exposes TNC and daemon components for use by other crates.

// Diagnostics go through `tracing`, never straight to stdout/stderr.
#![cfg_attr(not(test), deny(clippy::print_stdout, clippy::print_stderr))]

pub mod audio_output;
pub mod build_info;
pub mod config;
pub mod event_loop;
pub mod level_meter;
pub mod logging;
pub mod ptt_guardian;
#[cfg(feature = "websocket")]
pub mod spectrum;
pub mod startup_report;
#[cfg(feature = "kiss-tnc")]
pub mod tnc;
pub mod tx_chunker;
pub mod vara_wiring;
