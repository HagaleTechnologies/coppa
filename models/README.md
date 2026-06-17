# Channel-prediction models (placeholder)

This directory is a placeholder for optional channel-prediction model files.

**Coppa ships no machine-learning models and no inference runtime.** Channel
prediction is done by a plain EWMA (exponentially weighted moving average) predictor
with a linear trend, and MCS selection uses a static SNR-threshold lookup table. There
is no ONNX/neural-network inference in the codebase.

The `coppa-ml` crate contains an optional registry that scans this directory, but it
always falls back to the EWMA predictor. Wiring an actual inference runtime (e.g.
`tract` or `ort`) is **not implemented** and is out of scope for the reference
implementation.
