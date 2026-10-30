//! Matchable protocol failures.

/// Failure of protocol validation, framing, session control, ARQ or decompression.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// Unknown MAC frame type.
    #[error("Unknown MAC frame type: {value}")]
    UnknownMacFrameType { value: u8 },
    /// Callsign too long.
    #[error("Callsign too long: {len} chars (max {max})")]
    CallsignTooLong { len: usize, max: usize },
    /// Invalid callsign character.
    #[error("Invalid callsign character: '{character}'")]
    InvalidCallsignCharacter { character: char },
    /// Invalid callsign code.
    #[error("Invalid callsign code: {code}")]
    InvalidCallsignCode { code: u8 },
    /// MAC PDU too short.
    #[error("MAC PDU too short: {actual} bytes (min {min})")]
    MacPduTooShort { actual: usize, min: usize },
    /// Station-ID callsign exceeds the one-byte length prefix.
    #[error("StationIdPayload: callsign too long to encode ({len} bytes, max 255)")]
    StationIdCallsignTooLong { len: usize },
    /// Station-ID grid exceeds the one-byte length prefix.
    #[error("StationIdPayload: grid too long to encode ({len} bytes, max 255)")]
    StationIdGridTooLong { len: usize },
    /// Station-ID input contains no length prefix.
    #[error("StationIdPayload: empty input")]
    StationIdEmpty,
    /// Station-ID callsign or its following length prefix is missing.
    #[error("StationIdPayload: truncated callsign field")]
    StationIdTruncatedCallsign,
    /// Station-ID callsign bytes are not UTF-8.
    #[error("StationIdPayload: invalid callsign UTF-8: {source}")]
    StationIdCallsignUtf8 { source: std::string::FromUtf8Error },
    /// Station-ID grid or level byte is missing.
    #[error("StationIdPayload: truncated grid/level field")]
    StationIdTruncatedGridLevel,
    /// Station-ID grid bytes are not UTF-8.
    #[error("StationIdPayload: invalid grid UTF-8: {source}")]
    StationIdGridUtf8 { source: std::string::FromUtf8Error },
    /// Unknown app protocol.
    #[error("Unknown app protocol: {value}")]
    UnknownAppProtocol { value: u8 },
    /// No fragments to reassemble.
    #[error("No fragments to reassemble")]
    NoFragments,
    /// First fragment must have FragFlags.
    #[error("First fragment must have FragFlags::First")]
    MissingFirstFragment,
    /// Last fragment must have FragFlags.
    #[error("Last fragment must have FragFlags::Last")]
    MissingLastFragment,
    /// Fragment count mismatch.
    #[error("Fragment count mismatch: frag_total={declared} but got {actual} fragments")]
    FragmentCountMismatch { declared: u8, actual: usize },
    /// Fragment ID mismatch at index {index}.
    #[error("Fragment ID mismatch at index {index}: expected {expected}, got {actual}")]
    FragmentIdMismatch {
        index: usize,
        expected: u8,
        actual: u8,
    },
    /// Fragment index mismatch at position {position}.
    #[error("Fragment index mismatch at position {position}: expected {position}, got {actual}")]
    FragmentIndexMismatch { position: usize, actual: u8 },
    /// Fragment total mismatch at index {index}.
    #[error("Fragment total mismatch at index {index}: expected {expected}, got {actual}")]
    FragmentTotalMismatch {
        index: usize,
        expected: u8,
        actual: u8,
    },
    /// Wrong frag flags at index {index}.
    #[error("Wrong frag flags at index {index}: expected {expected:?}, got {actual:?}")]
    FragmentFlagsMismatch {
        index: usize,
        expected: crate::app::FragFlags,
        actual: crate::app::FragFlags,
    },
    /// App PDU too short.
    #[error("App PDU too short: {actual} bytes (min {min})")]
    AppPduTooShort { actual: usize, min: usize },
    /// Data too long.
    #[error("Data too long: {len} bytes (max {max})")]
    FrameDataTooLong { len: usize, max: usize },
    /// Bit sequence too short.
    #[error("Bit sequence too short")]
    FrameBitsTooShort,
    /// Bit sequence too short for sync detection.
    #[error("Bit sequence too short for sync detection")]
    SyncBitsTooShort,
    /// Sync word not found.
    #[error("Sync word not found")]
    SyncNotFound,
    /// Sync word not found in either polarity.
    #[error("Sync word not found in either polarity")]
    SyncNotFoundEitherPolarity,
    /// Payload too short.
    #[error("Payload too short")]
    FramePayloadTooShort,
    /// Truncated frame.
    #[error("Truncated frame: missing data or CRC")]
    FrameTruncated,
    /// CRC mismatch.
    #[error("CRC mismatch: expected 0x{expected:04X}, got 0x{actual:04X}")]
    FrameCrcMismatch { expected: u16, actual: u16 },
    /// Capabilities too short.
    #[error("Capabilities too short: {actual} bytes (need 7)")]
    CapabilitiesTooShort { actual: usize },
    /// Cannot initiate.
    #[error("Cannot initiate: session in state {state:?}")]
    CannotInitiate { state: crate::session::SessionState },
    /// CONNECT_ACK arrived outside Connecting.
    #[error("Unexpected CONNECT_ACK in state {state:?}")]
    UnexpectedConnectAck { state: crate::session::SessionState },
    /// CONNECT_REQ arrived outside Idle.
    #[error("Unexpected CONNECT_REQ in state {state:?}")]
    UnexpectedConnectReq { state: crate::session::SessionState },
    /// CONNECT_CFM arrived outside Accepting.
    #[error("Unexpected CONNECT_CFM in state {state:?}")]
    UnexpectedConnectCfm { state: crate::session::SessionState },
    /// Negotiated capabilities exceed intersection.
    #[error("Negotiated capabilities exceed intersection: payload={} (max {}), window={} (max {})", .negotiated.max_payload_size, .expected.max_payload_size, .negotiated.arq_window, .expected.arq_window)]
    CapabilitiesExceedIntersection {
        negotiated: crate::session::LinkCapabilities,
        expected: crate::session::LinkCapabilities,
    },
    /// Negotiated FEC rate {numerator}/{denominator} exceeds intersection {expected_numerator}/{expected_denominator}.
    #[error("Negotiated FEC rate {numerator}/{denominator} exceeds intersection {expected_numerator}/{expected_denominator}")]
    FecRateExceedsIntersection {
        numerator: u8,
        denominator: u8,
        expected_numerator: u8,
        expected_denominator: u8,
    },
    /// Negotiated compression enabled but not supported by both peers.
    #[error("Negotiated compression enabled but not supported by both peers")]
    CompressionNotMutuallySupported,
    /// Cannot disconnect.
    #[error("Cannot disconnect: session is idle")]
    CannotDisconnectIdle,
    /// Cannot send keepalive.
    #[error("Cannot send keepalive: session not established")]
    KeepaliveNotEstablished,
    /// No free session slots.
    #[error("No free session slots")]
    NoFreeSessionSlots,
    /// Window size must be 1-{max}, got {value}.
    #[error("Window size must be 1-{max}, got {value}")]
    InvalidWindowSize { value: u8, max: u8 },
    /// ARQ window full.
    #[error("ARQ window full")]
    ArqWindowFull,
    /// No pending segment with seq {seq}.
    #[error("No pending segment with seq {seq}")]
    NoPendingSegment { seq: u8 },
    /// LZ4 claimed decompressed size {claimed} exceeds limit {limit}.
    #[error("LZ4 claimed decompressed size {claimed} exceeds limit {limit}")]
    DecompressionSizeExceeded { claimed: usize, limit: usize },
    /// LZ4 decompression failed.
    #[error("LZ4 decompression failed: {source}")]
    Lz4Decompression {
        source: lz4_flex::block::DecompressError,
    },
    /// Unknown transport type.
    #[error("Unknown transport type: {value}")]
    UnknownTransportType { value: u8 },
    /// Transport PDU too short.
    #[error("Transport PDU too short: {actual} bytes (min {min})")]
    TransportPduTooShort { actual: usize, min: usize },
}
