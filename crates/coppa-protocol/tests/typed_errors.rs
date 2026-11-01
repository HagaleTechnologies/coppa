use coppa_protocol::{mac::Callsign, ProtocolError};
#[test]
fn callers_match_error_kind() {
    fn bounds<T: std::error::Error + Send + Sync + 'static>() {}
    bounds::<ProtocolError>();
    match Callsign::new("BAD!CHAR") {
        Err(ProtocolError::InvalidCallsignCharacter { character }) => assert_eq!(character, '!'),
        other => panic!("unexpected result: {other:?}"),
    }
}

use coppa_protocol::{
    app::{AppPdu, AppProtocol, Compression, FragFlags},
    arq::{ArqConfig, ArqTx},
    compression::lz4::lz4_decompress,
    mac::{MacFrameType, MacPdu, StationIdPayload},
    session::{LinkCapabilities, Session, SessionManager, SessionState},
    transport::{TransportPdu, TransportType},
    Frame,
};
use std::{
    error::Error,
    time::{Duration, Instant},
};

macro_rules! failure {
    ($call:expr, $pattern:pat, $display:expr) => {{
        let error = $call.unwrap_err();
        assert_eq!(error.to_string(), $display);
        assert!(matches!(error, $pattern), "unexpected error: {error:?}");
    }};
}

#[test]
fn discriminants_callsigns_and_short_pdus() {
    failure!(
        MacFrameType::from_u8(0xFF),
        ProtocolError::UnknownMacFrameType { value: 15 },
        "Unknown MAC frame type: 15"
    );
    failure!(
        TransportType::from_u8(0xFF),
        ProtocolError::UnknownTransportType { value: 15 },
        "Unknown transport type: 15"
    );
    failure!(
        AppProtocol::from_u8(0xFF),
        ProtocolError::UnknownAppProtocol { value: 15 },
        "Unknown app protocol: 15"
    );
    failure!(
        Callsign::new("123456789"),
        ProtocolError::CallsignTooLong { len: 9, max: 8 },
        "Callsign too long: 9 chars (max 8)"
    );
    failure!(
        Callsign::decode(&[0xFC, 0, 0, 0, 0, 0]),
        ProtocolError::InvalidCallsignCode { code: 63 },
        "Invalid callsign code: 63"
    );
    failure!(
        MacPdu::from_bytes(&[]),
        ProtocolError::MacPduTooShort { actual: 0, min: 14 },
        "MAC PDU too short: 0 bytes (min 14)"
    );
    failure!(
        TransportPdu::from_bytes(&[]),
        ProtocolError::TransportPduTooShort { actual: 0, min: 7 },
        "Transport PDU too short: 0 bytes (min 7)"
    );
    failure!(
        AppPdu::from_bytes(&[]),
        ProtocolError::AppPduTooShort { actual: 0, min: 4 },
        "App PDU too short: 0 bytes (min 4)"
    );
}

#[test]
fn station_id_errors_keep_fields_and_sources() {
    let mut id = StationIdPayload {
        callsign: "A".repeat(256),
        grid: None,
        level: 1,
    };
    failure!(
        id.to_bytes(),
        ProtocolError::StationIdCallsignTooLong { len: 256 },
        "StationIdPayload: callsign too long to encode (256 bytes, max 255)"
    );
    id.callsign = "A".into();
    id.grid = Some("X".repeat(256));
    failure!(
        id.to_bytes(),
        ProtocolError::StationIdGridTooLong { len: 256 },
        "StationIdPayload: grid too long to encode (256 bytes, max 255)"
    );
    failure!(
        StationIdPayload::from_bytes(&[]),
        ProtocolError::StationIdEmpty,
        "StationIdPayload: empty input"
    );
    failure!(
        StationIdPayload::from_bytes(&[1]),
        ProtocolError::StationIdTruncatedCallsign,
        "StationIdPayload: truncated callsign field"
    );
    failure!(
        StationIdPayload::from_bytes(&[0, 1]),
        ProtocolError::StationIdTruncatedGridLevel,
        "StationIdPayload: truncated grid/level field"
    );
    for (bytes, field) in [
        (vec![1, 255, 0, 1], "callsign"),
        (vec![0, 1, 255, 1], "grid"),
    ] {
        let e = StationIdPayload::from_bytes(&bytes).unwrap_err();
        let source = e
            .source()
            .unwrap()
            .downcast_ref::<std::string::FromUtf8Error>()
            .unwrap();
        assert_eq!(
            e.to_string(),
            format!("StationIdPayload: invalid {field} UTF-8: {source}")
        );
        match field {
            "callsign" => assert!(matches!(e, ProtocolError::StationIdCallsignUtf8 { .. })),
            _ => assert!(matches!(e, ProtocolError::StationIdGridUtf8 { .. })),
        }
    }
}

#[test]
fn fragment_failures_retain_positions_and_values() {
    failure!(
        AppPdu::reassemble(&[]),
        ProtocolError::NoFragments,
        "No fragments to reassemble"
    );
    let fragments = AppPdu::fragment(AppProtocol::Text, Compression::None, 42, b"abcdef", 2);
    let mut bad = fragments.clone();
    bad[0].frag_flags = FragFlags::Middle;
    failure!(
        AppPdu::reassemble(&bad),
        ProtocolError::MissingFirstFragment,
        "First fragment must have FragFlags::First"
    );
    let mut bad = fragments.clone();
    bad[2].frag_flags = FragFlags::Middle;
    failure!(
        AppPdu::reassemble(&bad),
        ProtocolError::MissingLastFragment,
        "Last fragment must have FragFlags::Last"
    );
    let mut bad = fragments.clone();
    bad[0].frag_total = 4;
    failure!(
        AppPdu::reassemble(&bad),
        ProtocolError::FragmentCountMismatch {
            declared: 4,
            actual: 3
        },
        "Fragment count mismatch: frag_total=4 but got 3 fragments"
    );
    let mut bad = fragments.clone();
    bad[1].frag_id = 43;
    failure!(
        AppPdu::reassemble(&bad),
        ProtocolError::FragmentIdMismatch {
            index: 1,
            expected: 42,
            actual: 43
        },
        "Fragment ID mismatch at index 1: expected 42, got 43"
    );
    let mut bad = fragments.clone();
    bad[1].frag_index = 7;
    failure!(
        AppPdu::reassemble(&bad),
        ProtocolError::FragmentIndexMismatch {
            position: 1,
            actual: 7
        },
        "Fragment index mismatch at position 1: expected 1, got 7"
    );
    let mut bad = fragments.clone();
    bad[1].frag_total = 7;
    failure!(
        AppPdu::reassemble(&bad),
        ProtocolError::FragmentTotalMismatch {
            index: 1,
            expected: 3,
            actual: 7
        },
        "Fragment total mismatch at index 1: expected 3, got 7"
    );
    let mut bad = fragments;
    bad[1].frag_flags = FragFlags::Complete;
    failure!(
        AppPdu::reassemble(&bad),
        ProtocolError::FragmentFlagsMismatch {
            index: 1,
            expected: FragFlags::Middle,
            actual: FragFlags::Complete
        },
        "Wrong frag flags at index 1: expected Middle, got Complete"
    );
}

#[test]
fn frame_failures_and_crc_values() {
    failure!(
        Frame::new(vec![0; 256]),
        ProtocolError::FrameDataTooLong { len: 256, max: 255 },
        "Data too long: 256 bytes (max 255)"
    );
    failure!(
        Frame::from_bits(&[]),
        ProtocolError::FrameBitsTooShort,
        "Bit sequence too short"
    );
    failure!(
        Frame::find_sync(&[]),
        ProtocolError::SyncBitsTooShort,
        "Bit sequence too short for sync detection"
    );
    failure!(
        Frame::find_sync(&[0; 256]),
        ProtocolError::SyncNotFound,
        "Sync word not found"
    );
    failure!(
        Frame::find_sync_with_polarity(&[0; 256]),
        ProtocolError::SyncNotFoundEitherPolarity,
        "Sync word not found in either polarity"
    );
    failure!(
        Frame::from_payload_bits(&[]),
        ProtocolError::FramePayloadTooShort,
        "Payload too short"
    );
    failure!(
        Frame::from_payload_bits(&[1; 24]),
        ProtocolError::FrameTruncated,
        "Truncated frame: missing data or CRC"
    );
    let (_, mut bits) = Frame::new(vec![42]).unwrap().to_bits_split().unwrap();
    *bits.last_mut().unwrap() ^= 1;
    let error = Frame::from_payload_bits(&bits).unwrap_err();
    match error {
        ProtocolError::FrameCrcMismatch { expected, actual } => {
            assert_eq!(expected ^ actual, 1);
            assert_eq!(
                error.to_string(),
                format!("CRC mismatch: expected 0x{expected:04X}, got 0x{actual:04X}")
            );
        }
        other => panic!("{other:?}"),
    }
}

fn session(caps: LinkCapabilities) -> Session {
    Session::new(
        0,
        Callsign::new("A").unwrap(),
        Callsign::new("B").unwrap(),
        0,
        caps,
    )
}

#[test]
fn session_state_errors_and_exhaustion() {
    failure!(
        LinkCapabilities::from_bytes(&[]),
        ProtocolError::CapabilitiesTooShort { actual: 0 },
        "Capabilities too short: 0 bytes (need 7)"
    );
    let mut s = session(LinkCapabilities::default());
    failure!(
        s.handle_connect_ack(&[]),
        ProtocolError::UnexpectedConnectAck {
            state: SessionState::Idle
        },
        "Unexpected CONNECT_ACK in state Idle"
    );
    failure!(
        s.handle_connect_cfm(&[]),
        ProtocolError::UnexpectedConnectCfm {
            state: SessionState::Idle
        },
        "Unexpected CONNECT_CFM in state Idle"
    );
    failure!(
        s.disconnect(),
        ProtocolError::CannotDisconnectIdle,
        "Cannot disconnect: session is idle"
    );
    failure!(
        s.keepalive(),
        ProtocolError::KeepaliveNotEstablished,
        "Cannot send keepalive: session not established"
    );
    s.initiate().unwrap();
    failure!(
        s.initiate(),
        ProtocolError::CannotInitiate {
            state: SessionState::Connecting
        },
        "Cannot initiate: session in state Connecting"
    );
    failure!(
        s.handle_connect_req(&[]),
        ProtocolError::UnexpectedConnectReq {
            state: SessionState::Connecting
        },
        "Unexpected CONNECT_REQ in state Connecting"
    );
    let mut manager = SessionManager::new();
    for _ in 0..16 {
        manager
            .create(
                Callsign::broadcast(),
                Callsign::broadcast(),
                0,
                LinkCapabilities::default(),
            )
            .unwrap();
    }
    failure!(
        manager.create(
            Callsign::broadcast(),
            Callsign::broadcast(),
            0,
            LinkCapabilities::default()
        ),
        ProtocolError::NoFreeSessionSlots,
        "No free session slots"
    );
}

#[test]
fn negotiated_capability_errors_retain_snapshots() {
    let caps = LinkCapabilities {
        compression: false,
        ..LinkCapabilities::default()
    };
    let mut s = session(caps.clone());
    s.handle_connect_req(&caps.to_bytes()).unwrap();
    let mut bad = caps.clone();
    bad.mcs_index += 1;
    let error = s.handle_connect_cfm(&bad.to_bytes()).unwrap_err();
    match &error {
        ProtocolError::CapabilitiesExceedIntersection {
            negotiated,
            expected,
        } => {
            assert_eq!(negotiated.mcs_index, bad.mcs_index);
            assert_eq!(expected.mcs_index, caps.mcs_index);
            assert_eq!(error.to_string(), format!("Negotiated capabilities exceed intersection: payload={} (max {}), window={} (max {})", bad.max_payload_size, caps.max_payload_size, bad.arq_window, caps.arq_window));
        }
        other => panic!("{other:?}"),
    }
    let mut bad = caps.clone();
    bad.fec_rate_num = 1;
    bad.fec_rate_den = 1;
    let e = s.handle_connect_cfm(&bad.to_bytes()).unwrap_err();
    assert!(
        matches!(e, ProtocolError::FecRateExceedsIntersection { numerator: 1, denominator: 1, expected_numerator, expected_denominator } if expected_numerator == caps.fec_rate_num && expected_denominator == caps.fec_rate_den)
    );
    assert_eq!(
        e.to_string(),
        format!(
            "Negotiated FEC rate 1/1 exceeds intersection {}/{}",
            caps.fec_rate_num, caps.fec_rate_den
        )
    );
    let mut bad = caps;
    bad.compression = true;
    failure!(
        s.handle_connect_cfm(&bad.to_bytes()),
        ProtocolError::CompressionNotMutuallySupported,
        "Negotiated compression enabled but not supported by both peers"
    );
}

#[test]
fn arq_and_decompression_failures() {
    failure!(
        ArqConfig::new(0, 1, Duration::from_secs(1)),
        ProtocolError::InvalidWindowSize { value: 0, max: 32 },
        "Window size must be 1-32, got 0"
    );
    let mut tx = ArqTx::new(ArqConfig::new(1, 1, Duration::from_secs(1)).unwrap());
    let now = Instant::now();
    tx.send(vec![1], now).unwrap();
    failure!(
        tx.send(vec![2], now),
        ProtocolError::ArqWindowFull,
        "ARQ window full"
    );
    failure!(
        tx.mark_retransmitted(42, now),
        ProtocolError::NoPendingSegment { seq: 42 },
        "No pending segment with seq 42"
    );
    failure!(
        lz4_decompress(&20_000u32.to_le_bytes()),
        ProtocolError::DecompressionSizeExceeded {
            claimed: 20_000,
            limit: 16_384
        },
        "LZ4 claimed decompressed size 20000 exceeds limit 16384"
    );
    let error = lz4_decompress(&[]).unwrap_err();
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<lz4_flex::block::DecompressError>()
        .unwrap();
    assert_eq!(
        error.to_string(),
        format!("LZ4 decompression failed: {source}")
    );
    assert!(matches!(error, ProtocolError::Lz4Decompression { .. }));
}
