//! Adaptive link-control logic for Coppa: channel-capacity-based speed-level
//! selection, the closed-loop sender-side rate controller, spread-gated
//! short-CP recommendation, and spectral occupancy sensing.
//!
//! None of this is model-based inference -- there is no model file, no
//! training, and no runtime that loads or executes one. Everything here is a
//! deterministic function of measurements this codebase's own receiver
//! already produces (per-carrier noise variances, delay-spread history, FFT
//! power spectra). Concretely, this crate provides:
//! - channel-capacity-based speed-level selection (`mcs::select_speed_level_2d` /
//!   `mcs::recommend_speed_level`), from real per-carrier noise variances, for
//!   picking a modulation/coding scheme,
//! - `RateLoop`, the sender-side closed-loop rate controller that applies a
//!   receiver-recommended speed level (fed back on the ACK) with hysteresis,
//! - `CpGate`, a spread-gated recommendation of whether the short-CP HF profile is
//!   currently safe to use, from measured per-frame delay-spread history,
//! - `BusyGate`, a spectral-occupancy transition gate over `SpectrumSensor`'s
//!   band-power estimate, used by the daemon's telemetry (Phase 3 Task 7),
//! - FFT-based spectrum sensing for noise-floor and occupancy estimation.
//!
//! An earlier version of this crate also carried an EWMA channel-quality
//! predictor and an optional-model-file registry (`channel_predictor.rs`,
//! `registry.rs`) that existed only to gesture at a future inference runtime
//! that was never built and had no real caller anywhere in this workspace;
//! Task 9 deleted both as dead code. See CLAUDE.md's Known Limitations for
//! what adaptation this crate does and does not perform today.

pub mod busy_gate;
pub mod cp_gate;
pub mod mcs;
pub mod rate_loop;
pub mod spectrum_sensor;

pub use busy_gate::BusyGate;
pub use cp_gate::{CpGate, CpRecommendation};
pub use mcs::{
    channel_capacity, channel_selectivity, recommend_speed_level, select_speed_level,
    select_speed_level_2d, select_speed_level_calibrated, SPEED_LEVEL_EFFICIENCY,
    SPEED_LEVEL_MIN_CAPACITY,
};
pub use rate_loop::{RateLoop, VALID_SPEED_LEVELS};
pub use spectrum_sensor::SpectrumSensor;
