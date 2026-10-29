//! Coppa daemon library: the `coppad` event loop and the components it runs
//! (config, logging, audio output, level metering, PTT guardian, TX chunking,
//! VARA wiring, ARDOP wiring, startup report), shared with the `coppad`
//! binary and `coppa-cli`. The KISS TNC (`tnc`) needs the `kiss-tnc` feature,
//! the spectrum stream (`spectrum`) needs `websocket`, and the ARDOP host
//! wiring (`ardop_wiring`) needs `ardop`; none is on by default.

// Diagnostics go through `tracing`, never straight to stdout/stderr.
#![cfg_attr(not(test), deny(clippy::print_stdout, clippy::print_stderr))]

#[cfg(feature = "ardop")]
pub mod ardop_wiring;
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
