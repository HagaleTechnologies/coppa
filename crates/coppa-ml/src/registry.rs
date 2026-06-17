//! Optional model-file discovery, with EWMA fallback.
//!
//! This scans a directory for optional `.onnx` files and records their paths,
//! but coppa-ml has no inference runtime: the files are never loaded or
//! executed. [`ModelRegistry::load_predictor`] always returns the
//! deterministic EWMA predictor. The scan exists only so a future runtime
//! could be wired in without changing callers.

use crate::{ChannelPredictor, EwmaPredictor};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A discovered (but never loaded) model file.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub model_type: String,
    pub version: String,
}

/// Scans a directory for optional model files and always falls back to EWMA.
///
/// No file discovered here is ever loaded or executed; the registry only
/// records paths. [`ModelRegistry::load_predictor`] returns the EWMA
/// predictor in all cases.
pub struct ModelRegistry {
    model_dir: PathBuf,
    models: Vec<ModelInfo>,
}

impl ModelRegistry {
    /// Create a new registry scanning the given directory.
    pub fn new<P: AsRef<Path>>(model_dir: P) -> Self {
        let model_dir = model_dir.as_ref().to_path_buf();
        let models = Self::scan_directory(&model_dir);
        Self { model_dir, models }
    }

    /// Scan directory for model files (.onnx).
    fn scan_directory(dir: &Path) -> Vec<ModelInfo> {
        let mut models = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("onnx") {
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    models.push(ModelInfo {
                        name: name.clone(),
                        path,
                        model_type: "onnx".to_string(),
                        version: "unknown".to_string(),
                    });
                }
            }
        }

        models
    }

    /// List all discovered models.
    pub fn list_models(&self) -> &[ModelInfo] {
        &self.models
    }

    /// Number of models found.
    pub fn count(&self) -> usize {
        self.models.len()
    }

    /// Get model info by name.
    pub fn get_model(&self, name: &str) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.name == name)
    }

    /// Return a channel predictor.
    ///
    /// Always returns the deterministic EWMA predictor. Any discovered model
    /// files are ignored, since there is no inference runtime to load them.
    pub fn load_predictor(&self) -> Result<Box<dyn ChannelPredictor>> {
        Ok(Box::new(EwmaPredictor::new(0.3, 20.0)))
    }

    /// Rescan the model directory for new files.
    pub fn refresh(&mut self) {
        self.models = Self::scan_directory(&self.model_dir);
    }

    /// Get the model directory path.
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_empty_dir() {
        let dir = std::env::temp_dir().join("coppa_models_test_empty");
        let _ = std::fs::create_dir_all(&dir);

        let registry = ModelRegistry::new(&dir);
        assert_eq!(registry.count(), 0);
        assert!(registry.list_models().is_empty());
        assert!(registry.get_model("anything").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_registry_fallback_predictor() {
        let dir = std::env::temp_dir().join("coppa_models_test_fallback");
        let _ = std::fs::create_dir_all(&dir);

        let registry = ModelRegistry::new(&dir);
        let pred = registry.load_predictor().unwrap();
        assert_eq!(pred.model_type(), "ewma-predictor");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_registry_with_onnx_files() {
        let dir = std::env::temp_dir().join("coppa_models_test_scan");
        let _ = std::fs::create_dir_all(&dir);

        // Create fake .onnx files
        std::fs::write(dir.join("predictor_v1.onnx"), b"fake").unwrap();
        std::fs::write(dir.join("predictor_v2.onnx"), b"fake").unwrap();
        std::fs::write(dir.join("not_a_model.txt"), b"fake").unwrap();

        let registry = ModelRegistry::new(&dir);
        assert_eq!(registry.count(), 2);

        let model = registry.get_model("predictor_v1");
        assert!(model.is_some());
        assert_eq!(model.unwrap().model_type, "onnx");

        // not_a_model.txt should not be found
        assert!(registry.get_model("not_a_model").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_registry_nonexistent_dir() {
        let registry = ModelRegistry::new("/tmp/coppa_nonexistent_dir_12345");
        assert_eq!(registry.count(), 0);
        // Should still give a fallback
        let pred = registry.load_predictor().unwrap();
        assert_eq!(pred.model_type(), "ewma-predictor");
    }

    #[test]
    fn test_registry_refresh() {
        let dir = std::env::temp_dir().join("coppa_models_test_refresh");
        let _ = std::fs::create_dir_all(&dir);

        let mut registry = ModelRegistry::new(&dir);
        assert_eq!(registry.count(), 0);

        // Add a model file
        std::fs::write(dir.join("new_model.onnx"), b"fake").unwrap();
        registry.refresh();
        assert_eq!(registry.count(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
