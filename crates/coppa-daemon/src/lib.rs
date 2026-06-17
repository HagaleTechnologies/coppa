//! Coppa daemon library — exposes TNC and daemon components for use by other crates.

pub mod config;
pub mod event_loop;
#[cfg(feature = "kiss-tnc")]
pub mod tnc;
