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
