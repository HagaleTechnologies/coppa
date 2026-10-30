use coppa_engine::{CoppaCore, EngineConfig, EngineError, OfdmBand, StreamFrame};
use coppa_protocol::modem::{ReceiveError, TransmitError};
use std::error::Error;

fn bounds<T: Error + Send + Sync + 'static>() {}

#[test]
fn payload_limits_keep_their_layer_and_source() {
    bounds::<EngineError>();
    let core = CoppaCore::new();
    let err = core.encode_bytes_with_level(&[0xAB; 4096], 1).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Transmit(TransmitError::PayloadTooLarge { max: 56 })
    ));
    assert!(err.source().unwrap().is::<TransmitError>());
    assert_eq!(
        err.to_string(),
        "payload too large for this speed level (max 56 bytes)"
    );
    let err = core.encode_bytes(&vec![0; 70_000]).unwrap_err();
    assert!(matches!(
        err,
        EngineError::PayloadTooLarge {
            len: 70_000,
            max: 65_535
        }
    ));
    assert_eq!(
        err.to_string(),
        "Payload too large (70000 bytes, max 65535)"
    );
}

#[test]
fn receive_and_squelch_failures_remain_distinct() {
    let core = CoppaCore::new();
    assert!(matches!(
        core.decode_bytes(&[]),
        Err(EngineError::NoSamplesToDecode)
    ));
    let tone: Vec<f32> = (0..48_000)
        .map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin() * 0.25)
        .collect();
    let err = core.decode_bytes(&tone).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Receive(ReceiveError::SyncFailed)
    ));
    assert!(err.source().unwrap().is::<ReceiveError>());
    assert_eq!(err.to_string(), "preamble synchronization failed");
    let squelched = CoppaCore::with_config(EngineConfig {
        squelch_threshold_db: -40.0,
        ..Default::default()
    });
    assert!(matches!(
        squelched.decode_bytes(&[]),
        Err(EngineError::NoSignalDetected)
    ));
}

#[test]
fn invalid_setters_leave_configuration_untouched() {
    let mut core = CoppaCore::new();
    for level in std::iter::once(0).chain(10..=255) {
        assert!(
            matches!(core.set_speed_level(level), Err(EngineError::InvalidSpeedLevel { level: actual }) if actual == level)
        );
        assert_eq!(core.speed_level(), 1);
    }
    let mut hf = CoppaCore::from_profile(&coppa_engine::HF_STANDARD);
    assert!(matches!(
        hf.set_speed_level(9),
        Err(EngineError::SpeedLevelUnavailableOnHf { level: 9 })
    ));
    assert_eq!(hf.speed_level(), coppa_engine::HF_STANDARD.speed_level);
    core.set_speed_level(9).unwrap();
    assert!(matches!(
        core.set_ofdm_band(OfdmBand::Hf),
        Err(EngineError::SpeedLevelUnavailableOnHf { level: 9 })
    ));
    assert_eq!(core.config().ofdm_band, OfdmBand::LevelDerived);
}

#[test]
fn invalid_utf8_is_only_a_text_decode_failure() {
    let core = CoppaCore::new();
    let samples = core.encode_bytes(&[0xff, 0xfe]).unwrap();
    assert_eq!(core.decode_bytes(&samples).unwrap(), [0xff, 0xfe]);
    let err = core.decode(&samples).unwrap_err();
    assert!(matches!(err, EngineError::InvalidUtf8(_)));
    assert!(err.source().unwrap().is::<std::string::FromUtf8Error>());
    assert_eq!(err.to_string(), err.source().unwrap().to_string());
}

#[test]
fn malformed_compression_surfaces_in_batch_and_streaming_results() {
    let tx = CoppaCore::new();
    let mut rx = CoppaCore::with_config(EngineConfig {
        compression_enabled: true,
        ..Default::default()
    });
    // A valid PHY frame containing the compression marker and an invalid LZ4 block.
    let samples = tx.encode_bytes(&[0xfe, 1, 0, 0, 0]).unwrap();
    let err = rx.decode_bytes(&samples).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Protocol(coppa_protocol::ProtocolError::Lz4Decompression { .. })
    ));
    assert!(err.source().unwrap().is::<coppa_protocol::ProtocolError>());
    assert_eq!(err.to_string(), err.source().unwrap().to_string());
    let mut padded = vec![0.0; 4800];
    padded.extend(samples);
    padded.extend(vec![0.0; 9600]);
    let frames: Vec<StreamFrame> = padded
        .chunks(480)
        .flat_map(|chunk| rx.push_samples(chunk))
        .collect();
    assert_eq!(frames.len(), 1);
    for frame in frames {
        let result: Result<Vec<u8>, EngineError> = frame.payload;
        assert!(matches!(
            result,
            Err(EngineError::Protocol(
                coppa_protocol::ProtocolError::Lz4Decompression { .. }
            ))
        ));
    }
}
