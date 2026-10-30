//! Errors returned by codec operations.

/// No failure kinds are currently defined: BPSK accepts empty and short input.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CodecError {}
