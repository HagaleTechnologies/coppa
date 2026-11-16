//! COP-170: `coppa-bench compare`, a same-simulator comparison harness.
//!
//! It runs coppa through the WGN, MPG and MPP channels at the SNR grid of the
//! 2020 Winlink comparison (30 to 0 dB) for 2 KB and 16 KB transfers, records
//! every trial in a CSV whose bytes depend only on the code and the seed,
//! and renders per-cell means with 95% Student t intervals. Only coppa rows
//! are measured here (COP-170 rescope, 2026-11-09).

pub mod blackbox;
pub mod channel_check;
pub mod grid;
pub mod link_stack;
pub mod render;
pub mod runner;
pub mod stats;
pub mod trial;

use std::path::{Path, PathBuf};

/// The repository root, resolved from this crate's manifest at build time.
#[must_use]
pub fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .map_or_else(|| manifest.join("../.."), Path::to_path_buf)
}

/// The published page: `<repo>/docs/benchmarks/comparison.md`.
#[must_use]
pub fn default_page() -> PathBuf {
    repo_root()
        .join("docs")
        .join("benchmarks")
        .join("comparison.md")
}

/// The published out-dir: `<repo>/results/compare`.
#[must_use]
pub fn default_out_dir() -> PathBuf {
    repo_root().join("results").join("compare")
}
