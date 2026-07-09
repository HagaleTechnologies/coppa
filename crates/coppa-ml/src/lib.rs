//! Channel quality prediction (EWMA), capacity-based MCS/speed-level selection, and the
//! closed-loop sender-side rate controller for Coppa.
//!
//! Despite the crate name, this is not machine learning. It provides:
//! - an exponentially-weighted-moving-average (EWMA) channel-quality
//!   predictor with a simple linear trend extrapolation,
//! - channel-capacity-based speed-level selection (`mcs::select_speed_level_2d` /
//!   `mcs::recommend_speed_level`) for picking a modulation/coding scheme,
//! - `RateLoop`, the sender-side closed-loop rate controller that applies a
//!   receiver-recommended speed level (fed back on the ACK) with hysteresis,
//! - `CpGate`, a spread-gated recommendation of whether the short-CP HF profile is
//!   currently safe to use, from measured per-frame delay-spread history,
//! - FFT-based spectrum sensing for noise-floor and occupancy estimation.
//!
//! No model is ever loaded and there is no inference runtime. The registry
//! can scan for optional model files, but the code always falls back to the
//! deterministic EWMA predictor.

use anyhow::Result;

pub mod channel_predictor;
pub mod cp_gate;
pub mod mcs;
pub mod rate_loop;
pub mod registry;
pub mod spectrum_sensor;

pub use channel_predictor::EwmaPredictor;
pub use cp_gate::{CpGate, CpRecommendation};
pub use mcs::{
    channel_capacity, channel_selectivity, recommend_speed_level, select_speed_level,
    select_speed_level_2d, select_speed_level_calibrated, SPEED_LEVEL_EFFICIENCY,
    SPEED_LEVEL_MIN_CAPACITY,
};
pub use rate_loop::{RateLoop, VALID_SPEED_LEVELS};
pub use registry::ModelRegistry;
pub use spectrum_sensor::SpectrumSensor;

/// Base trait for channel predictors.
///
/// Exposes a type name, a version string, and a confidence value. These are
/// descriptive metadata only; no model is loaded behind this trait.
pub trait MlModel: Send + Sync {
    fn model_type(&self) -> &str;
    fn version(&self) -> &str;
    fn confidence(&self) -> f32;
}

/// Predicts future channel quality.
pub trait ChannelPredictor: MlModel {
    fn observe(&mut self, quality: f32);
    fn predict(&self, frames_ahead: usize) -> f32;
}

/// A no-op predictor that always returns a fixed quality estimate.
pub struct FixedPredictor {
    quality: f32,
}

impl FixedPredictor {
    pub fn new(quality: f32) -> Self {
        Self { quality }
    }
}

impl MlModel for FixedPredictor {
    fn model_type(&self) -> &str {
        "fixed-predictor"
    }

    fn version(&self) -> &str {
        "0.0.0"
    }

    fn confidence(&self) -> f32 {
        0.0
    }
}

impl ChannelPredictor for FixedPredictor {
    fn observe(&mut self, _quality: f32) {}

    fn predict(&self, _frames_ahead: usize) -> f32 {
        self.quality
    }
}

/// Construct a channel predictor.
///
/// This always returns the deterministic EWMA predictor. For forward
/// compatibility it first scans the `models/` directory (relative to CWD) for
/// an optional `channel_predictor.onnx` file; if one is present a note is
/// logged, but the file is never loaded or executed (there is no inference
/// runtime). The EWMA predictor is returned regardless.
pub fn load_channel_predictor() -> Result<Box<dyn ChannelPredictor>> {
    let model_dir = std::env::current_dir().unwrap_or_default().join("models");
    let registry = ModelRegistry::new(&model_dir);
    if let Some(info) = registry.get_model("channel_predictor") {
        eprintln!(
            "Found a model file at {:?}, but coppa-ml has no inference runtime; using the EWMA predictor",
            info.path
        );
    }
    Ok(Box::new(EwmaPredictor::new(0.3, 20.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_predictor_basics() {
        let mut pred = FixedPredictor::new(15.0);
        assert_eq!(pred.model_type(), "fixed-predictor");
        assert_eq!(pred.version(), "0.0.0");
        assert_eq!(pred.confidence(), 0.0);
        assert_eq!(pred.predict(0), 15.0);
        assert_eq!(pred.predict(10), 15.0);
        pred.observe(30.0);
        assert_eq!(pred.predict(0), 15.0);
    }

    #[test]
    fn test_load_channel_predictor() {
        let pred = load_channel_predictor().unwrap();
        assert_eq!(pred.model_type(), "ewma-predictor");
    }

    #[test]
    fn test_ml_model_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FixedPredictor>();
        assert_send_sync::<EwmaPredictor>();
    }
}
