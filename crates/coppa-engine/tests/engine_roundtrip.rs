//! Integration test: engine encode → decode roundtrip for each profile.

use coppa_engine::profiles::{Profile, EMERGENCY, HF_ROBUST, HF_STANDARD, VHF_FAST};
use coppa_engine::CoppaCore;

fn roundtrip_profile(profile: &Profile, message: &str) {
    let engine = CoppaCore::from_profile(profile);
    let samples = engine
        .encode(message)
        .unwrap_or_else(|e| panic!("{}: encode failed: {}", profile.name, e));
    assert!(
        !samples.is_empty(),
        "{}: encode produced no samples",
        profile.name
    );

    let decoded = engine
        .decode(&samples)
        .unwrap_or_else(|e| panic!("{}: decode failed: {}", profile.name, e));
    assert_eq!(decoded, message, "{}: roundtrip mismatch", profile.name);
}

#[test]
fn test_hf_robust_roundtrip() {
    roundtrip_profile(&HF_ROBUST, "HF robust test");
}

#[test]
fn test_hf_standard_roundtrip() {
    roundtrip_profile(&HF_STANDARD, "HF standard test");
}

#[test]
fn test_vhf_fast_roundtrip() {
    roundtrip_profile(&VHF_FAST, "VHF fast test");
}

#[test]
fn test_emergency_roundtrip() {
    roundtrip_profile(&EMERGENCY, "SOS");
}

#[test]
fn test_bytes_roundtrip() {
    let engine = CoppaCore::new();
    let data: Vec<u8> = (0..50).collect();
    let samples = engine.encode_bytes(&data).expect("encode_bytes failed");
    let decoded = engine.decode_bytes(&samples).expect("decode_bytes failed");
    assert_eq!(decoded, data);
}

#[test]
fn test_compression_roundtrip_per_profile() {
    // HF_STANDARD and VHF_FAST have compression enabled
    for profile in &[&HF_STANDARD, &VHF_FAST] {
        assert!(
            profile.compression,
            "{} should have compression enabled",
            profile.name
        );
        let engine = CoppaCore::from_profile(profile);
        let message = "Compressed payload test";
        let samples = engine
            .encode(message)
            .unwrap_or_else(|e| panic!("{}: encode failed: {}", profile.name, e));
        let decoded = engine
            .decode(&samples)
            .unwrap_or_else(|e| panic!("{}: decode failed: {}", profile.name, e));
        assert_eq!(
            decoded, message,
            "{}: compression roundtrip mismatch",
            profile.name
        );
    }
}

/// Feed `samples` (with silence around them) through a fresh HF streaming
/// receiver; return the decoded frames and the headers of failed payloads.
fn stream_through_fresh_receiver(
    samples: &[f32],
) -> (
    Vec<coppa_protocol::modem::DecodedFrame>,
    Vec<coppa_codec::ofdm::frame::CoppaHeader>,
) {
    use coppa_codec::ofdm::CoppaProfile;
    use coppa_protocol::modem::StreamingReceiver;
    let mut rx = StreamingReceiver::new(
        CoppaProfile::hf_standard(),
        coppa_engine::WIRE_FORMAT_VERSION,
    );
    let mut padded = vec![0.0f32; 4800];
    padded.extend_from_slice(samples);
    padded.extend(vec![0.0f32; 9600]);
    let frames = padded
        .chunks(480)
        .flat_map(|chunk| rx.push_samples(chunk))
        .collect();
    (frames, rx.take_failed_headers())
}

/// COP-168: `encode_bytes_tagged` stamps the PHY `seq_num`, the redundancy
/// version (low two bits of `fec_type`) and the codeword count on the frame.
/// A lone RV3 multi-codeword frame may not decode, so the header is read
/// from whichever list the receiver put it in.
#[test]
fn tagged_encode_sets_phy_seq_and_rv() {
    let core = CoppaCore::new();
    let data: Vec<u8> = (0..120u8).collect();
    let samples = core
        .encode_bytes_tagged(&data, 2, 3, 0x2B, 3)
        .expect("tagged encode at level 2, 3 codewords");
    let (frames, failed) = stream_through_fresh_receiver(&samples);
    let headers: Vec<_> = frames
        .iter()
        .map(|f| f.header.clone())
        .chain(failed)
        .collect();
    assert_eq!(headers.len(), 1, "exactly one frame heard: {headers:?}");
    assert_eq!(headers[0].seq_num, 0x2B);
    assert_eq!(headers[0].fec_type & 0x03, 3);
    assert_eq!(headers[0].codewords, 3);
    assert_eq!(headers[0].speed_level, 2);

    // Only the RV bits reach the wire.
    let samples = core
        .encode_bytes_tagged(&data[..60], 2, 1, 0x05, 0xFD)
        .unwrap();
    let (frames, failed) = stream_through_fresh_receiver(&samples);
    let header = frames
        .first()
        .map(|f| f.header.clone())
        .or_else(|| failed.first().cloned())
        .expect("the frame's header decodes");
    assert_eq!(header.fec_type, 1, "rv 0xFD must go on the wire as RV1");

    assert!(core.encode_bytes_tagged(&data, 2, 0, 0, 0).is_err());
    assert!(core.encode_bytes_tagged(&data, 0, 1, 0, 0).is_err());
}

/// COP-168: a tagged RV0 frame with `phy_seq` 0 is sample-identical to the
/// plain encode, and a tagged frame under another key carries the same
/// payload bytes, so every attempt of a segment is byte-identical above
/// the PHY.
#[test]
fn tagged_encode_payload_equals_plain_encode_payload() {
    for profile in [&HF_STANDARD, &HF_ROBUST] {
        let core = CoppaCore::from_profile(profile);
        let data: Vec<u8> = b"B2F-style tagged payload ".repeat(2);
        let plain = core
            .encode_bytes_with_level_and_codewords(&data, 2, 1)
            .unwrap();
        let tagged = core.encode_bytes_tagged(&data, 2, 1, 0, 0).unwrap();
        assert_eq!(
            plain, tagged,
            "{}: RV0/seq 0 must match the plain encode",
            profile.name
        );

        let (plain_frames, _) = stream_through_fresh_receiver(&plain);
        let other = core.encode_bytes_tagged(&data, 2, 1, 0x3A, 0).unwrap();
        let (tagged_frames, _) = stream_through_fresh_receiver(&other);
        assert_eq!(plain_frames.len(), 1, "{}", profile.name);
        assert_eq!(tagged_frames.len(), 1, "{}", profile.name);
        assert_eq!(tagged_frames[0].header.seq_num, 0x3A);
        assert_eq!(
            plain_frames[0].payload, tagged_frames[0].payload,
            "{}: payload bytes on the wire must match",
            profile.name
        );
    }
}
