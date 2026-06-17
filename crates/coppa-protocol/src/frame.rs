//! PHY Frame structure for Coppa protocol.
//!
//! Preserves backward compatibility with the original Frame struct while adding
//! MAC PDU awareness. The PHY frame format is:
//!
//! ```text
//! [Preamble 128b][Sync 16b][PHY Header 16b][MAC PDU][CRC-16]
//! ```
//!
//! The PHY header contains:
//! - Length (8 bits): payload length in bytes (0-255)
//! - Reserved (8 bits): reserved for future use
//!
//! The preamble and sync word are transmitted uncoded so the receiver can find
//! the frame boundary before invoking FEC decoding.

use anyhow::{anyhow, Result};
use crc::{Crc, CRC_16_IBM_SDLC};

/// Generate the 128-bit PN preamble at compile time using an LFSR.
/// Polynomial: x^7 + x + 1, seed = 1, output = LSB, 127 bits + 1 pad bit.
const fn make_pn_preamble() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let mut lfsr: u8 = 1;
    let mut i: usize = 0;
    while i < 127 {
        let out = lfsr & 1;
        let fb = (lfsr ^ (lfsr >> 6)) & 1;
        lfsr = (lfsr >> 1) | (fb << 6);
        if out != 0 {
            bytes[i / 8] |= 1 << (7 - (i % 8));
        }
        i += 1;
    }
    bytes
}

/// 128-bit PN preamble (m-sequence from x^7+x+1, padded to 128 bits).
/// Flat spectrum, impulse-like autocorrelation -- better than 0xAAAA for
/// timing recovery without triggering SSB ALC.
const PN_PREAMBLE: [u8; 16] = make_pn_preamble();

/// Basic frame structure for Coppa protocol.
///
/// Frame format: [Preamble][Sync Word][FEC-encoded: Length + Data + CRC]
/// - Preamble: 128 bits of PN m-sequence for timing recovery
/// - Sync Word: 16 bits (0xF68D) for frame detection
/// - Length: 8 bits indicating data length (0-255 bytes)
/// - Data: Variable length payload (0-255 bytes)
/// - CRC: 16 bits CRC-16 for error detection
///
/// The preamble and sync word are transmitted uncoded so the receiver can
/// find the frame boundary before invoking FEC decoding.
#[derive(Debug, Clone)]
pub struct Frame {
    pub data: Vec<u8>,
}

impl Frame {
    /// Legacy preamble constant (alternating 1010...). Retained for backward
    /// compatibility; new code should use the module-level `PN_PREAMBLE` array.
    pub const PREAMBLE: u32 = 0xAAAAAAAA;

    /// Sync word for frame detection (16 bits).
    /// Selected for low autocorrelation sidelobes among 16-bit candidates,
    /// providing reliable frame boundary detection in noisy channels.
    pub const SYNC_WORD: u16 = 0xF68D;

    /// Maximum data length in bytes.
    pub const MAX_DATA_LENGTH: usize = 255;

    /// CRC algorithm: CRC-16-IBM-SDLC.
    const CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);

    /// Create a new frame with the given data.
    pub fn new(data: Vec<u8>) -> Result<Self> {
        if data.len() > Self::MAX_DATA_LENGTH {
            return Err(anyhow!(
                "Data too long: {} bytes (max {})",
                data.len(),
                Self::MAX_DATA_LENGTH
            ));
        }
        Ok(Self { data })
    }

    /// Convert frame to bits for transmission (V1 format).
    ///
    /// Returns (header_bits, payload_bits) where:
    /// - header_bits = preamble + sync word (transmitted uncoded)
    /// - payload_bits = length + data + CRC (to be FEC-encoded before transmission)
    ///
    /// **CRC scope (V1):** The CRC-16 covers only the `data` bytes (not the
    /// length field). Compare with `to_bits_split_v2` where the CRC covers
    /// the PHY header (length + reserved) plus data.
    pub fn to_bits_split(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut header = Vec::new();
        let mut payload = Vec::new();

        // Header: preamble + sync word (uncoded)
        // 128 bits of PN m-sequence gives the DSP chain (RRC filter,
        // Costas loop, Gardner timing) enough symbols to settle before the sync word.
        for &byte in &PN_PREAMBLE {
            for i in (0..8).rev() {
                header.push((byte >> i) & 1);
            }
        }
        for i in (0..16).rev() {
            header.push(((Self::SYNC_WORD >> i) & 1) as u8);
        }

        // Payload: length + data + CRC (will be FEC-encoded)
        let length = self.data.len() as u8;
        for i in (0..8).rev() {
            payload.push((length >> i) & 1);
        }
        for byte in &self.data {
            for i in (0..8).rev() {
                payload.push((byte >> i) & 1);
            }
        }
        let crc = Self::calculate_crc(&self.data);
        for i in (0..16).rev() {
            payload.push(((crc >> i) & 1) as u8);
        }

        Ok((header, payload))
    }

    /// Convert frame to a flat bit sequence (backward-compatible, no FEC split).
    pub fn to_bits(&self) -> Result<Vec<u8>> {
        let (header, payload) = self.to_bits_split()?;
        let mut bits = header;
        bits.extend(payload);
        Ok(bits)
    }

    /// Parse hard-decided bits into a frame.
    pub fn from_bits(bits: &[u8]) -> Result<Self> {
        // Minimum: 16 preamble check + 16 sync + 8 length + 16 CRC = 56 bits
        // But preamble is 128 bits, so minimum practical: 128 + 16 + 8 + 16 = 168
        if bits.len() < 168 {
            return Err(anyhow!("Bit sequence too short"));
        }

        let sync_start = Self::find_sync(bits)?;
        let payload_start = sync_start + 16;
        Self::parse_payload(&bits[payload_start..])
    }

    /// Parse a payload bit sequence (length + data + CRC) into a frame.
    /// Used when FEC decoding has already been applied to the payload.
    pub fn from_payload_bits(payload_bits: &[u8]) -> Result<Self> {
        Self::parse_payload(payload_bits)
    }

    /// Maximum number of bit errors tolerated in sync word detection.
    const SYNC_MAX_ERRORS: u32 = 2;

    /// Find the sync word in the bit stream, requiring a valid preamble.
    /// Returns the bit index where the sync word starts.
    /// Tolerates up to SYNC_MAX_ERRORS bit errors in the sync word for
    /// robustness against noisy channels.
    pub fn find_sync(bits: &[u8]) -> Result<usize> {
        // Need at least 16 bits of preamble check + 16 bits sync word
        let min_preamble_check = 16;
        if bits.len() < min_preamble_check + 16 {
            return Err(anyhow!("Bit sequence too short for sync detection"));
        }

        let mut best_pos = None;
        let mut best_errors = u32::MAX;

        for start in min_preamble_check..=(bits.len() - 16) {
            let word = Self::bits_to_u16(&bits[start..start + 16]);
            let errors = (word ^ Self::SYNC_WORD).count_ones();
            if errors <= Self::SYNC_MAX_ERRORS {
                // Check however many preceding bits we have (up to 32)
                let check_len = start.min(32);
                if Self::validate_preamble(&bits[start - check_len..start], check_len) {
                    if errors == 0 {
                        return Ok(start); // Exact match, return immediately
                    }
                    if errors < best_errors {
                        best_errors = errors;
                        best_pos = Some(start);
                    }
                }
            }
        }

        best_pos.ok_or_else(|| anyhow!("Sync word not found"))
    }

    /// Find sync word, trying both normal and inverted polarity.
    /// Returns (sync_position, inverted) where inverted=true means all bits are flipped.
    pub fn find_sync_with_polarity(bits: &[u8]) -> Result<(usize, bool)> {
        // Try normal polarity first
        if let Ok(pos) = Self::find_sync(bits) {
            return Ok((pos, false));
        }

        // Try inverted polarity (180-degree Costas loop ambiguity)
        let inverted: Vec<u8> = bits.iter().map(|&b| b ^ 1).collect();
        if let Ok(pos) = Self::find_sync(&inverted) {
            return Ok((pos, true));
        }

        Err(anyhow!("Sync word not found in either polarity"))
    }

    // -- MAC PDU integration --

    /// Create a frame from a MAC PDU byte payload.
    ///
    /// This wraps a serialized MAC PDU into the PHY frame structure.
    pub fn from_mac_pdu(mac_bytes: &[u8]) -> Result<Self> {
        Self::new(mac_bytes.to_vec())
    }

    /// Extract the MAC PDU bytes from this frame.
    ///
    /// Returns the raw data bytes which can be parsed as a MAC PDU.
    pub fn mac_pdu_bytes(&self) -> &[u8] {
        &self.data
    }

    // -- PHY header helpers (evolved frame) --

    /// Serialize an evolved PHY frame with a 2-byte PHY header.
    ///
    /// PHY header layout:
    /// - Byte 0: payload length (0-255)
    /// - Byte 1: reserved (set to 0)
    ///
    /// Returns (header_bits, payload_bits) for FEC split encoding.
    ///
    /// **CRC scope (V2):** The CRC-16 covers the PHY header bytes (length +
    /// reserved) AND the data bytes, unlike V1 where only data is covered.
    #[cfg(test)]
    pub fn to_bits_split_v2(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut header = Vec::new();
        let mut payload = Vec::new();

        // Preamble (128 bits of PN m-sequence)
        for &byte in &PN_PREAMBLE {
            for i in (0..8).rev() {
                header.push((byte >> i) & 1);
            }
        }
        // Sync word (16 bits)
        for i in (0..16).rev() {
            header.push(((Self::SYNC_WORD >> i) & 1) as u8);
        }

        // PHY header: length (8 bits) + reserved (8 bits)
        let length = self.data.len() as u8;
        for i in (0..8).rev() {
            payload.push((length >> i) & 1);
        }
        // Reserved byte
        payload.resize(payload.len() + 8, 0);

        // MAC PDU payload
        for byte in &self.data {
            for i in (0..8).rev() {
                payload.push((byte >> i) & 1);
            }
        }

        // CRC-16 over the whole payload (PHY header + data)
        let mut crc_input = vec![length, 0x00];
        crc_input.extend_from_slice(&self.data);
        let crc = Self::CRC.checksum(&crc_input);
        for i in (0..16).rev() {
            payload.push(((crc >> i) & 1) as u8);
        }

        Ok((header, payload))
    }

    /// Parse a V2 payload (PHY header + MAC PDU + CRC).
    #[cfg(test)]
    pub fn from_payload_bits_v2(payload_bits: &[u8]) -> Result<Self> {
        // Minimum: 8 (length) + 8 (reserved) + 0 (data) + 16 (CRC) = 32
        if payload_bits.len() < 32 {
            return Err(anyhow!("V2 payload too short"));
        }

        let mut idx = 0;

        // PHY header
        let length = Self::bits_to_u8(&payload_bits[idx..idx + 8]);
        idx += 8;
        let _reserved = Self::bits_to_u8(&payload_bits[idx..idx + 8]);
        idx += 8;

        let data_bits_needed = length as usize * 8;
        if idx + data_bits_needed + 16 > payload_bits.len() {
            return Err(anyhow!("Truncated V2 frame: missing data or CRC"));
        }

        let mut data = Vec::with_capacity(length as usize);
        for _ in 0..length {
            data.push(Self::bits_to_u8(&payload_bits[idx..idx + 8]));
            idx += 8;
        }

        // Verify CRC over (PHY header bytes + data)
        let received_crc = Self::bits_to_u16(&payload_bits[idx..idx + 16]);
        let mut crc_input = vec![length, 0x00];
        crc_input.extend_from_slice(&data);
        let calculated_crc = Self::CRC.checksum(&crc_input);

        if received_crc != calculated_crc {
            return Err(anyhow!(
                "V2 CRC mismatch: expected 0x{:04X}, got 0x{:04X}",
                calculated_crc,
                received_crc
            ));
        }

        Ok(Self { data })
    }

    // -- Internal helpers --

    /// Expand PN_PREAMBLE into a 128-element bit array for correlation.
    fn pn_preamble_bits() -> [u8; 128] {
        let mut out = [0u8; 128];
        for (byte_idx, &byte) in PN_PREAMBLE.iter().enumerate() {
            for bit in 0..8 {
                out[byte_idx * 8 + bit] = (byte >> (7 - bit)) & 1;
            }
        }
        out
    }

    /// Check whether a bit slice correlates with the expected PN preamble.
    /// Allows up to 25% bit errors for noise tolerance, with a minimum check
    /// length of 16 bits to avoid accepting random data.
    fn validate_preamble(bits: &[u8], check_len: usize) -> bool {
        if check_len < 16 {
            return false;
        }
        let pn_bits = Self::pn_preamble_bits();
        let mut errors = 0u32;
        let check = check_len.min(128);
        for (i, &bit) in bits.iter().rev().take(check).enumerate() {
            // i=0 is the last preamble bit (index 127)
            let expected = pn_bits[127 - i];
            if bit != expected {
                errors += 1;
            }
        }
        let max_errors = check as u32 / 4;
        errors <= max_errors
    }

    fn parse_payload(bits: &[u8]) -> Result<Self> {
        if bits.len() < 24 {
            // minimum: 8 (length) + 0 (data) + 16 (CRC) = 24
            return Err(anyhow!("Payload too short"));
        }

        let mut bit_index = 0;

        // Read length
        let length = Self::bits_to_u8(&bits[bit_index..bit_index + 8]);
        bit_index += 8;

        // Read data
        let data_bits_needed = length as usize * 8;
        if bit_index + data_bits_needed + 16 > bits.len() {
            return Err(anyhow!("Truncated frame: missing data or CRC"));
        }

        let mut data = Vec::new();
        for _ in 0..length {
            let byte = Self::bits_to_u8(&bits[bit_index..bit_index + 8]);
            data.push(byte);
            bit_index += 8;
        }

        // Read and verify CRC
        let received_crc = Self::bits_to_u16(&bits[bit_index..bit_index + 16]);
        let calculated_crc = Self::calculate_crc(&data);

        if received_crc != calculated_crc {
            return Err(anyhow!(
                "CRC mismatch: expected 0x{:04X}, got 0x{:04X}",
                calculated_crc,
                received_crc
            ));
        }

        Ok(Self { data })
    }

    fn bits_to_u8(bits: &[u8]) -> u8 {
        let mut value = 0u8;
        for (i, &bit) in bits.iter().take(8).enumerate() {
            if bit != 0 {
                value |= 1 << (7 - i);
            }
        }
        value
    }

    fn bits_to_u16(bits: &[u8]) -> u16 {
        let mut value = 0u16;
        for (i, &bit) in bits.iter().take(16).enumerate() {
            if bit != 0 {
                value |= 1 << (15 - i);
            }
        }
        value
    }

    fn calculate_crc(data: &[u8]) -> u16 {
        Self::CRC.checksum(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Original backward-compatible tests --

    #[test]
    fn test_frame_creation() {
        let data = b"Hello".to_vec();
        let frame = Frame::new(data.clone()).unwrap();
        assert_eq!(frame.data, data);
    }

    #[test]
    fn test_frame_too_long() {
        let data = vec![0u8; 256];
        assert!(Frame::new(data).is_err());
    }

    #[test]
    fn test_frame_roundtrip() {
        let original_data = b"Hello, World!".to_vec();
        let frame = Frame::new(original_data.clone()).unwrap();
        let bits = frame.to_bits().unwrap();
        let decoded_frame = Frame::from_bits(&bits).unwrap();
        assert_eq!(decoded_frame.data, original_data);
    }

    #[test]
    fn test_frame_split_roundtrip() {
        let original_data = b"FEC test".to_vec();
        let frame = Frame::new(original_data.clone()).unwrap();
        let (header, payload) = frame.to_bits_split().unwrap();

        // Header should be 144 bits (128 preamble + 16 sync)
        assert_eq!(header.len(), 144);

        // Payload parsed directly
        let decoded = Frame::from_payload_bits(&payload).unwrap();
        assert_eq!(decoded.data, original_data);
    }

    #[test]
    fn test_empty_frame() {
        let frame = Frame::new(Vec::new()).unwrap();
        let bits = frame.to_bits().unwrap();
        let decoded_frame = Frame::from_bits(&bits).unwrap();
        assert_eq!(decoded_frame.data, Vec::<u8>::new());
    }

    #[test]
    fn test_corrupted_crc() {
        let frame = Frame::new(b"Test".to_vec()).unwrap();
        let mut bits = frame.to_bits().unwrap();
        if let Some(last_bit) = bits.last_mut() {
            *last_bit = 1 - *last_bit;
        }
        assert!(Frame::from_bits(&bits).is_err());
    }

    #[test]
    fn test_sync_false_positive_in_data() {
        let data = vec![0x5A, 0x5A, 0x5A, 0x5A, 0x01, 0x02];
        let frame = Frame::new(data.clone()).unwrap();
        let bits = frame.to_bits().unwrap();
        let decoded = Frame::from_bits(&bits).unwrap();
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn test_sync_requires_preamble() {
        // All-zero bits before sync (not alternating) -- should be rejected
        let mut bits = vec![0u8; 32];
        for i in (0..16).rev() {
            bits.push(((Frame::SYNC_WORD >> i) & 1) as u8);
        }
        for i in (0..8).rev() {
            bits.push((1u8 >> i) & 1);
        }
        for i in (0..8).rev() {
            bits.push((0x42u8 >> i) & 1);
        }
        let crc = Frame::calculate_crc(&[0x42]);
        for i in (0..16).rev() {
            bits.push(((crc >> i) & 1) as u8);
        }
        assert!(Frame::from_bits(&bits).is_err());
    }

    #[test]
    fn test_polarity_detection() {
        let frame = Frame::new(b"Polarity".to_vec()).unwrap();
        let bits = frame.to_bits().unwrap();

        // Normal polarity should work
        let (pos, inv) = Frame::find_sync_with_polarity(&bits).unwrap();
        assert!(!inv);
        assert_eq!(pos, 128);

        // Inverted bits should also be detected
        let inverted: Vec<u8> = bits.iter().map(|&b| b ^ 1).collect();
        let (_, inv) = Frame::find_sync_with_polarity(&inverted).unwrap();
        assert!(inv);
    }

    // -- Constants verification --

    #[test]
    fn test_constants() {
        // PN preamble should be 16 bytes (128 bits)
        assert_eq!(PN_PREAMBLE.len(), 16);
        // The m-sequence should produce non-trivial output
        assert_ne!(PN_PREAMBLE, [0u8; 16]);
        assert_ne!(PN_PREAMBLE, [0xFFu8; 16]);
        assert_eq!(Frame::SYNC_WORD, 0xF68D);
        assert_eq!(Frame::MAX_DATA_LENGTH, 255);
    }

    // -- MAC PDU integration tests --

    #[test]
    fn test_from_mac_pdu() {
        let mac_bytes = vec![0x10, 0x01, 0x02, 0x03];
        let frame = Frame::from_mac_pdu(&mac_bytes).unwrap();
        assert_eq!(frame.mac_pdu_bytes(), &mac_bytes[..]);
    }

    #[test]
    fn test_mac_pdu_roundtrip_through_bits() {
        let mac_bytes = vec![0x10, 0xDE, 0xAD, 0xBE, 0xEF];
        let frame = Frame::from_mac_pdu(&mac_bytes).unwrap();
        let bits = frame.to_bits().unwrap();
        let decoded = Frame::from_bits(&bits).unwrap();
        assert_eq!(decoded.mac_pdu_bytes(), &mac_bytes[..]);
    }

    // -- V2 (evolved PHY header) tests --

    #[test]
    fn test_v2_roundtrip() {
        let data = b"V2 frame test".to_vec();
        let frame = Frame::new(data.clone()).unwrap();
        let (_header, payload) = frame.to_bits_split_v2().unwrap();
        let decoded = Frame::from_payload_bits_v2(&payload).unwrap();
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn test_v2_empty() {
        let frame = Frame::new(Vec::new()).unwrap();
        let (_header, payload) = frame.to_bits_split_v2().unwrap();
        let decoded = Frame::from_payload_bits_v2(&payload).unwrap();
        assert!(decoded.data.is_empty());
    }

    #[test]
    fn test_v2_max_length() {
        let data = vec![0xAB; 255];
        let frame = Frame::new(data.clone()).unwrap();
        let (_header, payload) = frame.to_bits_split_v2().unwrap();
        let decoded = Frame::from_payload_bits_v2(&payload).unwrap();
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn test_v2_crc_corruption() {
        let frame = Frame::new(b"CRC test".to_vec()).unwrap();
        let (_header, mut payload) = frame.to_bits_split_v2().unwrap();
        // Flip last bit (CRC)
        let last = payload.len() - 1;
        payload[last] ^= 1;
        assert!(Frame::from_payload_bits_v2(&payload).is_err());
    }

    #[test]
    fn test_v2_header_bits_size() {
        let frame = Frame::new(b"size".to_vec()).unwrap();
        let (header, payload) = frame.to_bits_split_v2().unwrap();
        // Header: 128 preamble + 16 sync = 144
        assert_eq!(header.len(), 144);
        // Payload: 8 len + 8 reserved + 4*8 data + 16 CRC = 64
        assert_eq!(payload.len(), 8 + 8 + 4 * 8 + 16);
    }

    #[test]
    fn test_v2_payload_too_short() {
        let short = vec![0u8; 16]; // less than 32 bits minimum
        assert!(Frame::from_payload_bits_v2(&short).is_err());
    }

    // -- Backward compatibility: old and new coexist --

    #[test]
    fn test_v1_and_v2_independent() {
        let data = b"compat".to_vec();
        let frame = Frame::new(data.clone()).unwrap();

        // V1 path
        let (_, v1_payload) = frame.to_bits_split().unwrap();
        let d1 = Frame::from_payload_bits(&v1_payload).unwrap();

        // V2 path
        let (_, v2_payload) = frame.to_bits_split_v2().unwrap();
        let d2 = Frame::from_payload_bits_v2(&v2_payload).unwrap();

        assert_eq!(d1.data, data);
        assert_eq!(d2.data, data);
    }

    // -- Error path tests --

    #[test]
    fn test_truncated_frame_too_short_for_header() {
        // Less than 168 bits minimum for from_bits
        let short_bits = vec![0u8; 40];
        assert!(Frame::from_bits(&short_bits).is_err());
    }

    #[test]
    fn test_payload_too_short() {
        // Less than 24 bits (8 length + 16 CRC minimum)
        let short_payload = vec![0u8; 16];
        assert!(Frame::from_payload_bits(&short_payload).is_err());
    }

    #[test]
    fn test_crc_mismatch() {
        let frame = Frame::new(b"CRC".to_vec()).unwrap();
        let (_, mut payload) = frame.to_bits_split().unwrap();
        // Flip a data bit (not CRC) to cause mismatch
        if payload.len() > 10 {
            payload[10] ^= 1;
        }
        assert!(Frame::from_payload_bits(&payload).is_err());
    }

    #[test]
    fn test_max_payload_exceeded() {
        let data = vec![0u8; 256]; // one more than MAX_DATA_LENGTH
        assert!(Frame::new(data).is_err());
    }

    #[test]
    fn test_zero_length_payload() {
        let frame = Frame::new(Vec::new()).unwrap();
        let (_, payload) = frame.to_bits_split().unwrap();
        let decoded = Frame::from_payload_bits(&payload).unwrap();
        assert!(decoded.data.is_empty());
    }

    #[test]
    fn test_corrupted_sync_word() {
        let frame = Frame::new(b"Sync".to_vec()).unwrap();
        let mut bits = frame.to_bits().unwrap();
        // Corrupt the sync word area (bits 128-143)
        for i in 128..144 {
            if i < bits.len() {
                bits[i] = 0;
            }
        }
        assert!(Frame::from_bits(&bits).is_err());
    }

    #[test]
    fn test_invalid_length_field() {
        // Create a valid frame then corrupt the length field in payload
        let frame = Frame::new(b"AB".to_vec()).unwrap();
        let (_, mut payload) = frame.to_bits_split().unwrap();
        // Set length to 255 (much larger than actual data)
        for b in payload.iter_mut().take(8) {
            *b = 1; // 0xFF = 255
        }
        // This should fail because there's not enough data for 255 bytes
        assert!(Frame::from_payload_bits(&payload).is_err());
    }

    #[test]
    fn test_no_sync_in_random_data() {
        let random_bits = vec![0u8; 200];
        assert!(Frame::find_sync(&random_bits).is_err());
    }

    #[test]
    fn test_polarity_no_sync_both() {
        let bits = vec![0u8; 100];
        assert!(Frame::find_sync_with_polarity(&bits).is_err());
    }
}
