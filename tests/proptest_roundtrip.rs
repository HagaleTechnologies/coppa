//! Property-based tests for Coppa using proptest.

use coppa_engine::CoppaCore;
use coppa_protocol::mac::Callsign;
use coppa_protocol::Frame;
use proptest::prelude::*;

proptest! {
    /// Any short message should roundtrip through encode/decode.
    #[test]
    fn prop_encode_decode_roundtrip(msg in "[A-Za-z0-9 ]{1,50}") {
        let core = CoppaCore::new();
        let samples = core.encode(&msg).unwrap();
        let decoded = core.decode(&samples).unwrap();
        prop_assert_eq!(msg, decoded);
    }

    /// Frame creation from arbitrary bytes should not panic.
    #[test]
    fn prop_frame_from_bytes_no_panic(data in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = Frame::new(data);
    }

    /// Callsign encoding should roundtrip for valid callsigns.
    #[test]
    fn prop_callsign_roundtrip(call in "[A-Z0-9]{1,8}") {
        let cs = Callsign::new(&call).unwrap();
        let encoded = cs.encode();
        let decoded = Callsign::decode(&encoded).unwrap();
        // Decoded callsign is padded with spaces, so trim
        let decoded_str = decoded.to_string();
        let trimmed = decoded_str.trim();
        prop_assert_eq!(&call[..], trimmed);
    }

    /// Binary payloads should roundtrip through encode_bytes/decode_bytes.
    #[test]
    fn prop_binary_roundtrip(data in prop::collection::vec(any::<u8>(), 1..50)) {
        let encoder = CoppaCore::new();
        let samples = encoder.encode_bytes(&data)
            .map_err(|e| TestCaseError::fail(format!("encode_bytes failed: {}", e)))?;
        let decoder = CoppaCore::new();
        let decoded = decoder.decode_bytes(&samples)
            .map_err(|e| TestCaseError::fail(format!("decode_bytes failed: {}", e)))?;
        prop_assert_eq!(&data, &decoded);
    }

    /// Frame bit serialization should roundtrip for valid payloads.
    #[test]
    fn prop_frame_bits_roundtrip(data in prop::collection::vec(any::<u8>(), 1..200)) {
        let frame = Frame::new(data.clone()).unwrap();
        let (_header_bits, payload_bits) = frame.to_bits_split().unwrap();

        // Payload bits from a valid frame should always roundtrip
        let recovered = Frame::from_payload_bits(&payload_bits)
            .map_err(|e| TestCaseError::fail(format!("Failed to recover frame: {}", e)))?;
        prop_assert_eq!(data, recovered.data);
    }
}

#[cfg(test)]
mod deterministic_tests {
    use super::*;

    #[test]
    fn test_short_messages_roundtrip() {
        let core = CoppaCore::new();
        for len in 1..=10 {
            let msg = "A".repeat(len);
            let samples = core.encode(&msg).unwrap();
            let decoded = core.decode(&samples).unwrap();
            assert_eq!(msg, decoded, "Failed at length {}", len);
        }
    }

    #[test]
    fn test_special_characters() {
        let core = CoppaCore::new();
        let msg = "VK2ABC/P de VK3DEF 599 599";
        let samples = core.encode(msg).unwrap();
        let decoded = core.decode(&samples).unwrap();
        assert_eq!(msg, decoded);
    }
}
