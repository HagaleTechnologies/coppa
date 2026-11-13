//! Matchable protocol failures.

/// Failure of protocol validation, framing, session control, ARQ or decompression.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// CONNECT_CFM disagrees with mutual floor support.
    #[error("robust floor capability mismatch")]
    RobustFloorMismatch,
    /// Invalid outer-code packet header or length.
    #[error("Malformed outer-code packet")]
    MalformedOuterPacket,
    /// CONNECT_CFM did not confirm the outer-code intersection.
    #[error("Outer-code capabilities do not match negotiation")]
    OuterCodeMismatch,
    /// COP-121: CONNECT_CFM did not confirm the `app_messages` AND.
    #[error("Application-message capability does not match negotiation")]
    AppMessagesMismatch,
    /// COP-121: a chat message has no non-whitespace character.
    #[error("Message is empty")]
    ChatEmpty,
    /// COP-121: a chat message is longer than the limit, in UTF-8 bytes.
    #[error("Message is {len} bytes; the limit is {max}")]
    ChatTooLong {
        /// Length of the message, in UTF-8 bytes.
        len: usize,
        /// Longest message accepted (`app_stream::CHAT_MAX_BYTES`), in bytes.
        max: usize,
    },
    /// COP-121: a stream 1 record length is shorter than the `AppPdu` header.
    #[error("Application record too short: length {len} (min 4)")]
    AppRecordTooShort {
        /// The record's length field.
        len: u16,
    },
    /// COP-121: an `AppPdu` is too long for a record's 16-bit length field.
    #[error("Application record too long: {len} bytes (max 65535)")]
    AppRecordTooLong {
        /// Length of the encoded `AppPdu`, in bytes.
        len: usize,
    },
    /// Unknown MAC frame type.
    #[error("Unknown MAC frame type: {value}")]
    UnknownMacFrameType {
        /// Frame-type byte read from the PDU header.
        value: u8,
    },
    /// Callsign too long.
    #[error("Callsign too long: {len} chars (max {max})")]
    CallsignTooLong {
        /// Length of the upper-cased callsign the caller passed, in bytes.
        len: usize,
        /// Longest callsign the 6-byte encoding holds (`MAX_CALLSIGN_LEN`), in characters.
        max: usize,
    },
    /// Invalid callsign character.
    #[error("Invalid callsign character: '{character}'")]
    InvalidCallsignCharacter {
        /// First character (after upper-casing) outside `A-Z`, `0-9`, `/` and `-`.
        character: char,
    },
    /// Invalid callsign code.
    #[error("Invalid callsign code: {code}")]
    InvalidCallsignCode {
        /// Encoded byte that maps to no callsign character.
        code: u8,
    },
    /// MAC PDU too short.
    #[error("MAC PDU too short: {actual} bytes (min {min})")]
    MacPduTooShort {
        /// Length of the input, in bytes.
        actual: usize,
        /// Shortest valid MAC PDU, in bytes.
        min: usize,
    },
    /// Station-ID callsign exceeds the one-byte length prefix.
    #[error("StationIdPayload: callsign too long to encode ({len} bytes, max 255)")]
    StationIdCallsignTooLong {
        /// Callsign length in bytes; the prefix holds at most 255.
        len: usize,
    },
    /// Station-ID grid exceeds the one-byte length prefix.
    #[error("StationIdPayload: grid too long to encode ({len} bytes, max 255)")]
    StationIdGridTooLong {
        /// Grid-locator length in bytes; the prefix holds at most 255.
        len: usize,
    },
    /// Station-ID input contains no length prefix.
    #[error("StationIdPayload: empty input")]
    StationIdEmpty,
    /// Station-ID callsign or its following length prefix is missing.
    #[error("StationIdPayload: truncated callsign field")]
    StationIdTruncatedCallsign,
    /// Station-ID callsign bytes are not UTF-8.
    #[error("StationIdPayload: invalid callsign UTF-8: {source}")]
    StationIdCallsignUtf8 {
        /// UTF-8 decoding error, which also holds the rejected bytes.
        source: std::string::FromUtf8Error,
    },
    /// Station-ID grid or level byte is missing.
    #[error("StationIdPayload: truncated grid/level field")]
    StationIdTruncatedGridLevel,
    /// Station-ID grid bytes are not UTF-8.
    #[error("StationIdPayload: invalid grid UTF-8: {source}")]
    StationIdGridUtf8 {
        /// UTF-8 decoding error, which also holds the rejected bytes.
        source: std::string::FromUtf8Error,
    },
    /// Unknown app protocol.
    #[error("Unknown app protocol: {value}")]
    UnknownAppProtocol {
        /// Protocol byte read from the application PDU header.
        value: u8,
    },
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
    FragmentCountMismatch {
        /// `frag_total` carried in the fragments' headers.
        declared: u8,
        /// Number of fragments the caller supplied.
        actual: usize,
    },
    /// Fragment ID mismatch at index {index}.
    #[error("Fragment ID mismatch at index {index}: expected {expected}, got {actual}")]
    FragmentIdMismatch {
        /// Position of the offending fragment in the input, from 0.
        index: usize,
        /// Fragment ID of the first fragment, which every fragment must share.
        expected: u8,
        /// Fragment ID found at `index`.
        actual: u8,
    },
    /// Fragment index mismatch at position {position}.
    #[error("Fragment index mismatch at position {position}: expected {position}, got {actual}")]
    FragmentIndexMismatch {
        /// Position in the input, from 0; also the expected `frag_index`.
        position: usize,
        /// `frag_index` found at `position`.
        actual: u8,
    },
    /// Fragment total mismatch at index {index}.
    #[error("Fragment total mismatch at index {index}: expected {expected}, got {actual}")]
    FragmentTotalMismatch {
        /// Position of the offending fragment in the input, from 0.
        index: usize,
        /// `frag_total` of the first fragment, which every fragment must share.
        expected: u8,
        /// `frag_total` found at `index`.
        actual: u8,
    },
    /// Wrong frag flags at index {index}.
    #[error("Wrong frag flags at index {index}: expected {expected:?}, got {actual:?}")]
    FragmentFlagsMismatch {
        /// Position of the offending fragment in the input, from 0.
        index: usize,
        /// Flags a fragment at this position must carry.
        expected: crate::app::FragFlags,
        /// Flags found at `index`.
        actual: crate::app::FragFlags,
    },
    /// App PDU too short.
    #[error("App PDU too short: {actual} bytes (min {min})")]
    AppPduTooShort {
        /// Length of the input, in bytes.
        actual: usize,
        /// Shortest valid application PDU, in bytes.
        min: usize,
    },
    /// Data too long.
    #[error("Data too long: {len} bytes (max {max})")]
    FrameDataTooLong {
        /// Length of the data the caller passed, in bytes.
        len: usize,
        /// Largest payload one PHY frame carries, in bytes.
        max: usize,
    },
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
    FrameCrcMismatch {
        /// CRC-16 computed over the received length byte and data.
        expected: u16,
        /// CRC-16 carried in the frame.
        actual: u16,
    },
    /// Capabilities too short.
    #[error("Capabilities too short: {actual} bytes (need 7)")]
    CapabilitiesTooShort {
        /// Length of the input, in bytes; the encoding needs 7.
        actual: usize,
    },
    /// Cannot initiate.
    #[error("Cannot initiate: session in state {state:?}")]
    CannotInitiate {
        /// State the session was in; only `Idle` can initiate.
        state: crate::session::SessionState,
    },
    /// CONNECT_ACK arrived outside Connecting.
    #[error("Unexpected CONNECT_ACK in state {state:?}")]
    UnexpectedConnectAck {
        /// State the session was in when the PDU arrived.
        state: crate::session::SessionState,
    },
    /// CONNECT_REQ arrived outside Idle.
    #[error("Unexpected CONNECT_REQ in state {state:?}")]
    UnexpectedConnectReq {
        /// State the session was in when the PDU arrived.
        state: crate::session::SessionState,
    },
    /// CONNECT_CFM arrived outside Accepting.
    #[error("Unexpected CONNECT_CFM in state {state:?}")]
    UnexpectedConnectCfm {
        /// State the session was in when the PDU arrived.
        state: crate::session::SessionState,
    },
    /// Negotiated capabilities exceed intersection.
    #[error("Negotiated capabilities exceed intersection: payload={} (max {}), window={} (max {})", .negotiated.max_payload_size, .expected.max_payload_size, .negotiated.arq_window, .expected.arq_window)]
    CapabilitiesExceedIntersection {
        /// Capabilities the peer's CONNECT_CFM proposed.
        negotiated: crate::session::LinkCapabilities,
        /// Intersection of both stations' capabilities, the most it may propose.
        expected: crate::session::LinkCapabilities,
    },
    /// Negotiated FEC rate {numerator}/{denominator} exceeds intersection {expected_numerator}/{expected_denominator}.
    #[error("Negotiated FEC rate {numerator}/{denominator} exceeds intersection {expected_numerator}/{expected_denominator}")]
    FecRateExceedsIntersection {
        /// Numerator of the FEC rate the peer proposed.
        numerator: u8,
        /// Denominator of the FEC rate the peer proposed.
        denominator: u8,
        /// Numerator of the highest rate both stations support.
        expected_numerator: u8,
        /// Denominator of the highest rate both stations support.
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
    InvalidWindowSize {
        /// Window size the caller passed, in segments.
        value: u8,
        /// Largest window accepted (`arq::MAX_WINDOW_SIZE`), in segments.
        max: u8,
    },
    /// ARQ window full.
    #[error("ARQ window full")]
    ArqWindowFull,
    /// No pending segment with seq {seq}.
    #[error("No pending segment with seq {seq}")]
    NoPendingSegment {
        /// Sequence number the caller asked for.
        seq: u8,
    },
    /// LZ4 claimed decompressed size {claimed} exceeds limit {limit}.
    #[error("LZ4 claimed decompressed size {claimed} exceeds limit {limit}")]
    DecompressionSizeExceeded {
        /// Decompressed size in the LZ4 header, in bytes.
        claimed: usize,
        /// Largest decompressed size accepted, in bytes.
        limit: usize,
    },
    /// LZ4 decompression failed.
    #[error("LZ4 decompression failed: {source}")]
    Lz4Decompression {
        /// Error the LZ4 block decoder reported.
        source: lz4_flex::block::DecompressError,
    },
    /// Unknown transport type.
    #[error("Unknown transport type: {value}")]
    UnknownTransportType {
        /// Type byte read from the transport PDU header.
        value: u8,
    },
    /// Transport PDU too short.
    #[error("Transport PDU too short: {actual} bytes (min {min})")]
    TransportPduTooShort {
        /// Length of the input, in bytes.
        actual: usize,
        /// Shortest valid transport PDU, in bytes.
        min: usize,
    },
}
