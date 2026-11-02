//! Coppa - Open-source ham radio digital communications system.
//!
//! This is the root crate that re-exports sub-crates for convenience.
//! For direct use, depend on individual crates (coppa-engine, coppa-codec, etc.).

pub use coppa_codec as codec;
pub use coppa_dsp as dsp;
pub use coppa_engine as engine;
pub use coppa_protocol as protocol;

// Runs the embedding guide's Rust blocks as doctests, so the guide keeps compiling.
#[cfg(doctest)]
#[doc = include_str!("../docs/tutorials/embedding.md")]
struct EmbeddingGuideDoctests;
