//! Channel quality prediction (EWMA) and MCS selection for Coppa.
//!
//! Despite the crate name, this is not machine learning. It provides:
//! - an exponentially-weighted-moving-average (EWMA) channel-quality
//!   predictor with a simple linear trend extrapolation,
//! - a static SNR-to-MCS lookup table for picking a modulation/coding scheme,
//! - FFT-based spectrum sensing for noise-floor and occupancy estimation.
//!
//! No model is ever loaded and there is no inference runtime. The registry
//! can scan for optional model files, but the code always falls back to the
//! deterministic EWMA predictor.

use anyhow::Result;

pub mod channel_predictor;
pub mod mcs;
pub mod registry;
pub mod spectrum_sensor;

pub use channel_predictor::EwmaPredictor;
pub use mcs::{
    channel_capacity, select_mcs, select_speed_level, McsEntry, MCS_TABLE, SPEED_LEVEL_EFFICIENCY,
};
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

/// Predicts future channel quality and recommends modulation/coding schemes.
pub trait ChannelPredictor: MlModel {
    fn observe(&mut self, quality: f32);
    fn predict(&self, frames_ahead: usize) -> f32;
    fn recommend_mcs(&self) -> u8;
}

/// A no-op predictor that always returns a fixed quality estimate.
pub struct FixedPredictor {
    quality: f32,
    mcs: u8,
}

impl FixedPredictor {
    pub fn new(quality: f32, mcs: u8) -> Self {
        Self { quality, mcs }
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

    fn recommend_mcs(&self) -> u8 {
        self.mcs
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
        let mut pred = FixedPredictor::new(15.0, 3);
        assert_eq!(pred.model_type(), "fixed-predictor");
        assert_eq!(pred.version(), "0.0.0");
        assert_eq!(pred.confidence(), 0.0);
        assert_eq!(pred.predict(0), 15.0);
        assert_eq!(pred.predict(10), 15.0);
        assert_eq!(pred.recommend_mcs(), 3);
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
