//! Measurement harness for Coppa's PHY modes: BER / frame-error-rate / goodput vs SNR.

pub mod ground_truth;
pub mod metrics;
pub mod per_frame_link;
pub mod profile_ab;
pub mod report;
pub mod runner;
pub mod scenario;
pub mod transfer;
