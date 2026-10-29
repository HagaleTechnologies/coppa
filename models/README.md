# models/ (unused)

Nothing in the workspace reads this directory. `coppa-ml` once had an
optional model-file registry that scanned it, alongside an EWMA channel-quality
predictor; Phase 3 Task 9 deleted both as dead code (see
`docs/adr/008-phase3-system-layer.md`).

**Coppa ships no machine-learning models and no inference runtime.** Speed-level
selection is a deterministic function of measured per-carrier noise variances
(`coppa_ml::mcs`), applied by the closed-loop `RateLoop` controller.
