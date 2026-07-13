//! MAC layer Protocol Data Unit (PDU) for Coppa.
//!
//! ```text
//! MAC PDU → [Version 4b][FrameType 4b][Dest 6B][Src 6B][SSID 1B][Transport PDU]
//! ```
//!
//! Callsign encoding uses a 6-bit charset packing 8 characters into 6 bytes:
//! - A-Z = 1-26, 0-9 = 27-36, '/' = 37, '-' = 38, space/pad = 0

use anyhow::{anyhow, Result};

/// Current MAC protocol version.
pub const MAC_VERSION: u8 = 1;

/// Maximum callsign length in characters.
pub const MAX_CALLSIGN_LEN: usize = 8;

/// Encoded callsign size in bytes (8 chars * 6 bits = 48 bits = 6 bytes).
pub const ENCODED_CALLSIGN_BYTES: usize = 6;

/// MAC frame types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MacFrameType {
    Data = 0,
    Ack = 1,
    ConnectReq = 2,
    ConnectAck = 3,
    ConnectCfm = 4,
    Disconnect = 5,
    Beacon = 6,
    Keepalive = 7,
    Relay = 8,
}

impl MacFrameType {
    /// Convert from 4-bit nibble value.
    pub fn from_u8(val: u8) -> Result<Self> {
        match val & 0x0F {
            0 => Ok(MacFrameType::Data),
            1 => Ok(MacFrameType::Ack),
            2 => Ok(MacFrameType::ConnectReq),
            3 => Ok(MacFrameType::ConnectAck),
            4 => Ok(MacFrameType::ConnectCfm),
            5 => Ok(MacFrameType::Disconnect),
            6 => Ok(MacFrameType::Beacon),
            7 => Ok(MacFrameType::Keepalive),
            8 => Ok(MacFrameType::Relay),
            v => Err(anyhow!("Unknown MAC frame type: {}", v)),
        }
    }
}

/// A callsign encoded in the 6-bit Coppa charset.
///
/// 8 characters are packed into 6 bytes (48 bits) using:
/// - A-Z = 1..26
/// - 0-9 = 27..36
/// - '/' = 37
/// - '-' = 38
/// - pad = 0
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Callsign {
    /// The raw string (uppercase, max 8 chars).
    text: String,
}

impl Callsign {
    /// Create a callsign from a string, validating charset and length.
    pub fn new(s: &str) -> Result<Self> {
        let upper = s.to_uppercase();
        if upper.len() > MAX_CALLSIGN_LEN {
            return Err(anyhow!(
                "Callsign too long: {} chars (max {})",
                upper.len(),
                MAX_CALLSIGN_LEN
            ));
        }
        for ch in upper.chars() {
            if char_to_code(ch).is_none() {
                return Err(anyhow!("Invalid callsign character: '{}'", ch));
            }
        }
        Ok(Self { text: upper })
    }

    /// Create a broadcast/wildcard callsign (all zeros).
    pub fn broadcast() -> Self {
        Self {
            text: String::new(),
        }
    }

    /// Get the callsign string.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Encode callsign to 6 bytes (48 bits, 8 chars * 6 bits).
    pub fn encode(&self) -> [u8; ENCODED_CALLSIGN_BYTES] {
        let mut codes = [0u8; MAX_CALLSIGN_LEN];
        for (i, ch) in self.text.chars().enumerate() {
            codes[i] = char_to_code(ch).unwrap_or(0);
        }

        // Pack 8 x 6-bit values into 6 bytes (48 bits)
        let mut bytes = [0u8; ENCODED_CALLSIGN_BYTES];
        // bits: c0[5..0] c1[5..0] c2[5..0] c3[5..0] c4[5..0] c5[5..0] c6[5..0] c7[5..0]
        // byte0 = c0[5..0] c1[5..4]
        // byte1 = c1[3..0] c2[5..2]
        // byte2 = c2[1..0] c3[5..0]
        // byte3 = c4[5..0] c5[5..4]
        // byte4 = c5[3..0] c6[5..2]
        // byte5 = c6[1..0] c7[5..0]
        bytes[0] = (codes[0] << 2) | (codes[1] >> 4);
        bytes[1] = (codes[1] << 4) | (codes[2] >> 2);
        bytes[2] = (codes[2] << 6) | codes[3];
        bytes[3] = (codes[4] << 2) | (codes[5] >> 4);
        bytes[4] = (codes[5] << 4) | (codes[6] >> 2);
        bytes[5] = (codes[6] << 6) | codes[7];

        bytes
    }

    /// Decode callsign from 6 bytes.
    pub fn decode(bytes: &[u8; ENCODED_CALLSIGN_BYTES]) -> Result<Self> {
        let mut codes = [0u8; MAX_CALLSIGN_LEN];
        codes[0] = (bytes[0] >> 2) & 0x3F;
        codes[1] = ((bytes[0] & 0x03) << 4) | ((bytes[1] >> 4) & 0x0F);
        codes[2] = ((bytes[1] & 0x0F) << 2) | ((bytes[2] >> 6) & 0x03);
        codes[3] = bytes[2] & 0x3F;
        codes[4] = (bytes[3] >> 2) & 0x3F;
        codes[5] = ((bytes[3] & 0x03) << 4) | ((bytes[4] >> 4) & 0x0F);
        codes[6] = ((bytes[4] & 0x0F) << 2) | ((bytes[5] >> 6) & 0x03);
        codes[7] = bytes[5] & 0x3F;

        let mut text = String::new();
        for &code in &codes {
            if code == 0 {
                break; // padding
            }
            match code_to_char(code) {
                Some(ch) => text.push(ch),
                None => return Err(anyhow!("Invalid callsign code: {}", code)),
            }
        }

        Ok(Self { text })
    }
}

impl std::fmt::Display for Callsign {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.text)
    }
}

/// MAC Protocol Data Unit.
#[derive(Debug, Clone)]
pub struct MacPdu {
    /// Protocol version (4 bits, currently 1).
    pub version: u8,
    /// Frame type.
    pub frame_type: MacFrameType,
    /// Destination callsign.
    pub dest: Callsign,
    /// Source callsign.
    pub src: Callsign,
    /// SSID (secondary station ID, 0-15).
    pub ssid: u8,
    /// Transport PDU payload bytes.
    pub payload: Vec<u8>,
}

impl MacPdu {
    /// Minimum serialized size: 1 (ver+type) + 6 (dest) + 6 (src) + 1 (ssid) = 14 bytes.
    pub const HEADER_SIZE: usize = 14;

    /// Create a new MAC PDU for data transfer.
    pub fn new_data(dest: Callsign, src: Callsign, ssid: u8, payload: Vec<u8>) -> Self {
        Self {
            version: MAC_VERSION,
            frame_type: MacFrameType::Data,
            dest,
            src,
            ssid: ssid & 0x0F,
            payload,
        }
    }

    /// Create a new MAC PDU with a specific frame type.
    pub fn new(
        frame_type: MacFrameType,
        dest: Callsign,
        src: Callsign,
        ssid: u8,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            version: MAC_VERSION,
            frame_type,
            dest,
            src,
            ssid: ssid & 0x0F,
            payload,
        }
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::HEADER_SIZE + self.payload.len());

        // Byte 0: version (high nibble) | frame_type (low nibble)
        out.push(((self.version & 0x0F) << 4) | (self.frame_type as u8 & 0x0F));

        // Dest callsign (6 bytes)
        out.extend_from_slice(&self.dest.encode());

        // Src callsign (6 bytes)
        out.extend_from_slice(&self.src.encode());

        // SSID (1 byte, only low nibble used)
        out.push(self.ssid & 0x0F);

        // Transport PDU payload
        out.extend_from_slice(&self.payload);

        out
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < Self::HEADER_SIZE {
            return Err(anyhow!(
                "MAC PDU too short: {} bytes (min {})",
                bytes.len(),
                Self::HEADER_SIZE
            ));
        }

        let version = (bytes[0] >> 4) & 0x0F;
        let frame_type = MacFrameType::from_u8(bytes[0] & 0x0F)?;

        let mut dest_bytes = [0u8; ENCODED_CALLSIGN_BYTES];
        dest_bytes.copy_from_slice(&bytes[1..7]);
        let dest = Callsign::decode(&dest_bytes)?;

        let mut src_bytes = [0u8; ENCODED_CALLSIGN_BYTES];
        src_bytes.copy_from_slice(&bytes[7..13]);
        let src = Callsign::decode(&src_bytes)?;

        let ssid = bytes[13] & 0x0F;
        let payload = bytes[14..].to_vec();

        Ok(Self {
            version,
            frame_type,
            dest,
            src,
            ssid,
            payload,
        })
    }
}

/// Payload carried inside a [`MacFrameType::Beacon`] frame (station ID / beacon
/// mode, Phase 4 Task 3): the operator's callsign, an optional free-text grid
/// locator (e.g. "FN20"), and the speed level the frame itself was sent at.
///
/// This is distinct from -- and redundant with -- `MacPdu::src`'s packed 6-bit
/// callsign encoding: `src` is what a receiving station's MAC layer uses to
/// route/identify the frame, while this payload is the actual human-readable
/// identification *content* (what an operator or a logging tool would display),
/// matching the task brief's literal "callsign + grid (optional) + level"
/// payload spec. Kept deliberately simple (length-prefixed ASCII fields, no
/// compression, no versioning) since it's always sent as a small, level-1
/// single-codeword frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationIdPayload {
    /// Station callsign, e.g. "VK3ABC".
    pub callsign: String,
    /// Optional free-text Maidenhead grid locator, e.g. "FN20". No validation
    /// beyond what [`Self::to_bytes`]/[`Self::from_bytes`] need (this is an
    /// operator-supplied free-text field).
    pub grid: Option<String>,
    /// Speed level (1-10) this frame was encoded at.
    pub level: u8,
}

impl StationIdPayload {
    /// Serialize to bytes: `[callsign_len: u8][callsign][grid_len: u8][grid][level: u8]`.
    /// `grid_len == 0` means no grid was configured.
    ///
    /// Both length prefixes are single bytes, so a `callsign`/`grid` longer
    /// than 255 bytes can't be represented -- these are unvalidated
    /// operator-supplied config strings (real callsigns/grids are short in
    /// practice, but nothing upstream enforces that), so this returns an
    /// error rather than silently truncating the length prefix and emitting
    /// a corrupted frame.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let callsign_bytes = self.callsign.as_bytes();
        let grid_bytes = self.grid.as_deref().unwrap_or("").as_bytes();
        if callsign_bytes.len() > u8::MAX as usize {
            return Err(anyhow!(
                "StationIdPayload: callsign too long to encode ({} bytes, max 255)",
                callsign_bytes.len()
            ));
        }
        if grid_bytes.len() > u8::MAX as usize {
            return Err(anyhow!(
                "StationIdPayload: grid too long to encode ({} bytes, max 255)",
                grid_bytes.len()
            ));
        }
        let mut out = Vec::with_capacity(2 + callsign_bytes.len() + grid_bytes.len() + 1);
        out.push(callsign_bytes.len() as u8);
        out.extend_from_slice(callsign_bytes);
        out.push(grid_bytes.len() as u8);
        out.extend_from_slice(grid_bytes);
        out.push(self.level);
        Ok(out)
    }

    /// Deserialize from bytes produced by [`Self::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() {
            return Err(anyhow!("StationIdPayload: empty input"));
        }
        let callsign_len = bytes[0] as usize;
        let callsign_end = 1 + callsign_len;
        if bytes.len() < callsign_end + 1 {
            return Err(anyhow!("StationIdPayload: truncated callsign field"));
        }
        let callsign = String::from_utf8(bytes[1..callsign_end].to_vec())
            .map_err(|e| anyhow!("StationIdPayload: invalid callsign UTF-8: {e}"))?;

        let grid_len = bytes[callsign_end] as usize;
        let grid_start = callsign_end + 1;
        let grid_end = grid_start + grid_len;
        if bytes.len() < grid_end + 1 {
            return Err(anyhow!("StationIdPayload: truncated grid/level field"));
        }
        let grid = if grid_len == 0 {
            None
        } else {
            Some(
                String::from_utf8(bytes[grid_start..grid_end].to_vec())
                    .map_err(|e| anyhow!("StationIdPayload: invalid grid UTF-8: {e}"))?,
            )
        };
        let level = bytes[grid_end];

        Ok(Self {
            callsign,
            grid,
            level,
        })
    }
}

// ── 6-bit charset helpers ───────────────────────────────────────────

fn char_to_code(ch: char) -> Option<u8> {
    match ch {
        'A'..='Z' => Some(ch as u8 - b'A' + 1),
        '0'..='9' => Some(ch as u8 - b'0' + 27),
        '/' => Some(37),
        '-' => Some(38),
        _ => None,
    }
}

fn code_to_char(code: u8) -> Option<char> {
    match code {
        1..=26 => Some((b'A' + code - 1) as char),
        27..=36 => Some((b'0' + code - 27) as char),
        37 => Some('/'),
        38 => Some('-'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Callsign encoding tests ─────────────────────────────────────

    #[test]
    fn test_callsign_roundtrip() {
        for cs in &["VK3ABC", "W1AW", "JA1YAF", "N0CALL", "A", "ZZ9ZZZ"] {
            let callsign = Callsign::new(cs).unwrap();
            let encoded = callsign.encode();
            let decoded = Callsign::decode(&encoded).unwrap();
            assert_eq!(decoded.as_str(), cs.to_uppercase());
        }
    }

    #[test]
    fn test_callsign_case_insensitive() {
        let cs1 = Callsign::new("vk3abc").unwrap();
        let cs2 = Callsign::new("VK3ABC").unwrap();
        assert_eq!(cs1.encode(), cs2.encode());
    }

    #[test]
    fn test_callsign_max_length() {
        let cs = Callsign::new("VK3ABC-1").unwrap();
        assert_eq!(cs.as_str().len(), 8);
    }

    #[test]
    fn test_callsign_too_long() {
        assert!(Callsign::new("VK3ABCDEF").is_err());
    }

    #[test]
    fn test_callsign_invalid_char() {
        assert!(Callsign::new("VK3!ABC").is_err());
    }

    #[test]
    fn test_callsign_special_chars() {
        let cs = Callsign::new("VK3/P").unwrap();
        let encoded = cs.encode();
        let decoded = Callsign::decode(&encoded).unwrap();
        assert_eq!(decoded.as_str(), "VK3/P");

        let cs2 = Callsign::new("W1AW-5").unwrap();
        let encoded2 = cs2.encode();
        let decoded2 = Callsign::decode(&encoded2).unwrap();
        assert_eq!(decoded2.as_str(), "W1AW-5");
    }

    #[test]
    fn test_callsign_broadcast() {
        let bc = Callsign::broadcast();
        assert_eq!(bc.as_str(), "");
        let encoded = bc.encode();
        assert_eq!(encoded, [0u8; 6]);
        let decoded = Callsign::decode(&encoded).unwrap();
        assert_eq!(decoded.as_str(), "");
    }

    #[test]
    fn test_callsign_all_letters() {
        let cs = Callsign::new("ABCDEFGH").unwrap();
        let encoded = cs.encode();
        let decoded = Callsign::decode(&encoded).unwrap();
        assert_eq!(decoded.as_str(), "ABCDEFGH");
    }

    #[test]
    fn test_callsign_all_digits() {
        let cs = Callsign::new("01234567").unwrap();
        let encoded = cs.encode();
        let decoded = Callsign::decode(&encoded).unwrap();
        assert_eq!(decoded.as_str(), "01234567");
    }

    #[test]
    fn test_charset_coverage() {
        // Every valid character should roundtrip
        for ch in 'A'..='Z' {
            let code = char_to_code(ch).unwrap();
            assert_eq!(code_to_char(code).unwrap(), ch);
        }
        for ch in '0'..='9' {
            let code = char_to_code(ch).unwrap();
            assert_eq!(code_to_char(code).unwrap(), ch);
        }
        assert_eq!(code_to_char(char_to_code('/').unwrap()).unwrap(), '/');
        assert_eq!(code_to_char(char_to_code('-').unwrap()).unwrap(), '-');
    }

    // ── MAC frame type tests ────────────────────────────────────────

    #[test]
    fn test_frame_type_roundtrip() {
        let types = [
            MacFrameType::Data,
            MacFrameType::Ack,
            MacFrameType::ConnectReq,
            MacFrameType::ConnectAck,
            MacFrameType::ConnectCfm,
            MacFrameType::Disconnect,
            MacFrameType::Beacon,
            MacFrameType::Keepalive,
            MacFrameType::Relay,
        ];
        for &ft in &types {
            let val = ft as u8;
            let decoded = MacFrameType::from_u8(val).unwrap();
            assert_eq!(decoded, ft);
        }
    }

    #[test]
    fn test_frame_type_invalid() {
        assert!(MacFrameType::from_u8(9).is_err());
        assert!(MacFrameType::from_u8(15).is_err());
    }

    // ── MAC PDU tests ───────────────────────────────────────────────

    #[test]
    fn test_mac_pdu_roundtrip() {
        let dest = Callsign::new("VK3ABC").unwrap();
        let src = Callsign::new("W1AW").unwrap();
        let payload = vec![0x01, 0x02, 0x03, 0x04];
        let pdu = MacPdu::new_data(dest.clone(), src.clone(), 5, payload.clone());

        let bytes = pdu.to_bytes();
        let decoded = MacPdu::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.version, MAC_VERSION);
        assert_eq!(decoded.frame_type, MacFrameType::Data);
        assert_eq!(decoded.dest.as_str(), "VK3ABC");
        assert_eq!(decoded.src.as_str(), "W1AW");
        assert_eq!(decoded.ssid, 5);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn test_mac_pdu_all_types() {
        let dest = Callsign::new("N0CALL").unwrap();
        let src = Callsign::new("K1ABC").unwrap();

        for ft in [
            MacFrameType::Data,
            MacFrameType::Ack,
            MacFrameType::ConnectReq,
            MacFrameType::ConnectAck,
            MacFrameType::ConnectCfm,
            MacFrameType::Disconnect,
            MacFrameType::Beacon,
            MacFrameType::Keepalive,
            MacFrameType::Relay,
        ] {
            let pdu = MacPdu::new(ft, dest.clone(), src.clone(), 0, vec![]);
            let bytes = pdu.to_bytes();
            let decoded = MacPdu::from_bytes(&bytes).unwrap();
            assert_eq!(decoded.frame_type, ft);
        }
    }

    #[test]
    fn test_mac_pdu_empty_payload() {
        let pdu = MacPdu::new_data(
            Callsign::new("AA1AA").unwrap(),
            Callsign::new("BB2BB").unwrap(),
            0,
            vec![],
        );
        let bytes = pdu.to_bytes();
        assert_eq!(bytes.len(), MacPdu::HEADER_SIZE);
        let decoded = MacPdu::from_bytes(&bytes).unwrap();
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn test_mac_pdu_too_short() {
        let short = vec![0u8; 10];
        assert!(MacPdu::from_bytes(&short).is_err());
    }

    #[test]
    fn test_mac_pdu_ssid_masking() {
        let pdu = MacPdu::new_data(
            Callsign::new("A").unwrap(),
            Callsign::new("B").unwrap(),
            0xFF, // should be masked to 0x0F
            vec![],
        );
        assert_eq!(pdu.ssid, 0x0F);
    }

    #[test]
    fn test_mac_pdu_version() {
        let pdu = MacPdu::new_data(
            Callsign::new("A").unwrap(),
            Callsign::new("B").unwrap(),
            0,
            vec![],
        );
        let bytes = pdu.to_bytes();
        // Version in high nibble of first byte
        assert_eq!((bytes[0] >> 4) & 0x0F, MAC_VERSION);
    }

    #[test]
    fn test_mac_pdu_large_payload() {
        let payload = vec![0xAB; 200];
        let pdu = MacPdu::new_data(
            Callsign::new("LONG").unwrap(),
            Callsign::new("TEST").unwrap(),
            1,
            payload.clone(),
        );
        let bytes = pdu.to_bytes();
        let decoded = MacPdu::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.payload, payload);
    }

    // ── StationIdPayload tests (Phase 4 Task 3) ───────────────────────

    #[test]
    fn test_station_id_payload_roundtrip_with_grid() {
        let payload = StationIdPayload {
            callsign: "VK3ABC".to_string(),
            grid: Some("QF22".to_string()),
            level: 1,
        };
        let bytes = payload.to_bytes().unwrap();
        let decoded = StationIdPayload::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn test_station_id_payload_roundtrip_no_grid() {
        let payload = StationIdPayload {
            callsign: "W1AW".to_string(),
            grid: None,
            level: 1,
        };
        let bytes = payload.to_bytes().unwrap();
        let decoded = StationIdPayload::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(decoded.grid, None);
    }

    #[test]
    fn test_station_id_payload_encodes_level() {
        let payload = StationIdPayload {
            callsign: "N0CALL".to_string(),
            grid: None,
            level: 7,
        };
        let bytes = payload.to_bytes().unwrap();
        let decoded = StationIdPayload::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.level, 7);
    }

    #[test]
    fn test_station_id_payload_to_bytes_errors_on_oversized_callsign() {
        let payload = StationIdPayload {
            callsign: "X".repeat(256),
            grid: None,
            level: 1,
        };
        assert!(payload.to_bytes().is_err());
    }

    #[test]
    fn test_station_id_payload_to_bytes_errors_on_oversized_grid() {
        let payload = StationIdPayload {
            callsign: "VK3ABC".to_string(),
            grid: Some("G".repeat(256)),
            level: 1,
        };
        assert!(payload.to_bytes().is_err());
    }

    #[test]
    fn test_station_id_payload_from_bytes_empty_errors() {
        assert!(StationIdPayload::from_bytes(&[]).is_err());
    }

    #[test]
    fn test_station_id_payload_from_bytes_truncated_errors() {
        // Claims a 10-byte callsign but only provides 2 bytes of data.
        assert!(StationIdPayload::from_bytes(&[10, b'A', b'B']).is_err());
    }

    #[test]
    fn test_station_id_payload_as_beacon_mac_pdu_roundtrip() {
        let cs = Callsign::new("VK3ABC").unwrap();
        let id_payload = StationIdPayload {
            callsign: "VK3ABC".to_string(),
            grid: Some("QF22".to_string()),
            level: 1,
        };
        let pdu = MacPdu::new(
            MacFrameType::Beacon,
            cs.clone(),
            cs.clone(),
            0,
            id_payload.to_bytes().unwrap(),
        );
        let bytes = pdu.to_bytes();
        let decoded_pdu = MacPdu::from_bytes(&bytes).unwrap();
        assert_eq!(decoded_pdu.frame_type, MacFrameType::Beacon);
        assert_eq!(decoded_pdu.src.as_str(), "VK3ABC");
        assert_eq!(decoded_pdu.dest.as_str(), "VK3ABC");
        let decoded_id = StationIdPayload::from_bytes(&decoded_pdu.payload).unwrap();
        assert_eq!(decoded_id, id_payload);
    }

    #[test]
    fn test_mac_pdu_broadcast_dest() {
        let pdu = MacPdu::new(
            MacFrameType::Beacon,
            Callsign::broadcast(),
            Callsign::new("VK3ABC").unwrap(),
            0,
            vec![0x42],
        );
        let bytes = pdu.to_bytes();
        let decoded = MacPdu::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.dest.as_str(), "");
        assert_eq!(decoded.frame_type, MacFrameType::Beacon);
    }
}
