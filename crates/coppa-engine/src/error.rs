//! Matchable failures from the engine's encode and decode paths.

/// Engine validation failures and typed failures from its underlying pipeline.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// The payload exceeds the header's length field.
    #[error("Payload too large ({len} bytes, max {max})")]
    PayloadTooLarge {
        /// Payload length in bytes, after compression when it is enabled.
        len: usize,
        /// Largest payload the header's length field can carry, in bytes.
        max: usize,
    },
    /// The requested wire speed level is invalid.
    #[error("Invalid speed level: {level}")]
    InvalidSpeedLevel {
        /// The rejected level; valid levels are 1-9.
        level: u8,
    },
    /// The requested level requires a VHF profile.
    #[error("speed level {level} is not available on an HF profile: HF profiles carry levels 1-8 (COP-24)")]
    SpeedLevelUnavailableOnHf {
        /// The requested VHF-only level (9).
        level: u8,
    },
    /// Input power is below the configured squelch.
    #[error("No signal detected")]
    NoSignalDetected,
    /// Batch decoding was given no samples with squelch disabled.
    #[error("No samples to decode")]
    NoSamplesToDecode,
    /// The requested codeword count exceeds the frame limit.
    #[error("codewords ({requested}) exceeds max {max}")]
    TooManyCodewords {
        /// Codewords per frame the caller asked for.
        requested: u8,
        /// Most codewords one frame may carry.
        max: u8,
    },
    /// An explicit multi-codeword encode requires at least one codeword.
    #[error("codewords (0) is below the minimum of 1")]
    ZeroCodewords,
    /// The transceiver rejected the frame for transmission.
    #[error("{0}")]
    Transmit(#[from] coppa_protocol::modem::TransmitError),
    /// The transceiver could not receive a frame.
    #[error("{0}")]
    Receive(#[from] coppa_protocol::modem::ReceiveError),
    /// Decompression failed.
    #[error("{0}")]
    Protocol(#[from] coppa_protocol::ProtocolError),
    /// A text decode contained non-UTF-8 bytes.
    #[error("{0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}
