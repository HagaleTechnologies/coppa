//! Matchable failures from host servers and ARDOP framing.

/// A host I/O or framing failure.
///
/// Variants are available with every feature configuration. Match with a
/// wildcard so callers remain compatible with future failure kinds.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HostError {
    /// A listener, stream read, or stream write failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The host payload does not fit in the length prefix.
    #[error("host frame exceeds MAX_FRAME")]
    HostFrameTooLarge { len: usize, max: usize },
    /// The tag and TNC payload do not fit in the length prefix.
    #[error("TNC frame exceeds MAX_FRAME")]
    TncFrameTooLarge { len: usize, max: usize },
    /// The frame ends before its two-byte prefix.
    #[error("truncated frame: no length prefix")]
    MissingLengthPrefix,
    /// The declared frame cannot hold a three-byte tag.
    #[error("frame length {len} carries no tag")]
    MissingTag { len: usize },
    /// The declared body length differs from the available bytes.
    #[error("frame length {declared} does not match the {actual} bytes present")]
    FrameLengthMismatch { declared: usize, actual: usize },
    /// A received tag is not UTF-8.
    #[error("bad tag: {source}")]
    InvalidTagUtf8 { source: std::str::Utf8Error },
    /// A read length exceeds the frame allocation limit.
    #[error("frame length {len} exceeds MAX_FRAME")]
    FrameTooLarge { len: usize, max: usize },
}
