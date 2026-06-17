//! Application layer Protocol Data Unit for Coppa.
//!
//! ```text
//! Application PDU → [AppProto 4b][Compress 2b][FragFlags 2b][FragID 1B][Payload]
//! ```
//!
//! The application PDU supports protocol multiplexing, compression indication,
//! and fragmentation for payloads that exceed a single transport segment.

use anyhow::{anyhow, Result};

/// Application protocol identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AppProtocol {
    /// Plain text message.
    Text = 0,
    /// Position report (GPS coordinates, APRS-like).
    Position = 1,
    /// Telemetry data.
    Telemetry = 2,
    /// File transfer.
    FileTransfer = 3,
    /// Voice codec frame (Codec2 or similar).
    Voice = 4,
    /// Control / management messages.
    Control = 5,
    /// Custom / user-defined protocol.
    Custom = 6,
    /// Compressed data (protocol in payload header).
    CompressedData = 7,
}

impl AppProtocol {
    pub fn from_u8(val: u8) -> Result<Self> {
        match val & 0x0F {
            0 => Ok(AppProtocol::Text),
            1 => Ok(AppProtocol::Position),
            2 => Ok(AppProtocol::Telemetry),
            3 => Ok(AppProtocol::FileTransfer),
            4 => Ok(AppProtocol::Voice),
            5 => Ok(AppProtocol::Control),
            6 => Ok(AppProtocol::Custom),
            7 => Ok(AppProtocol::CompressedData),
            v => Err(anyhow!("Unknown app protocol: {}", v)),
        }
    }
}

/// Compression method indicator (2 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Compression {
    /// No compression.
    None = 0,
    /// DEFLATE (zlib).
    Deflate = 1,
    /// LZ4.
    Lz4 = 2,
    /// Reserved for future use.
    Reserved = 3,
}

impl Compression {
    pub fn from_u8(val: u8) -> Result<Self> {
        match val & 0x03 {
            0 => Ok(Compression::None),
            1 => Ok(Compression::Deflate),
            2 => Ok(Compression::Lz4),
            3 => Ok(Compression::Reserved),
            _ => unreachable!(),
        }
    }
}

/// Fragmentation flags (2 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FragFlags {
    /// Complete (unfragmented) PDU.
    Complete = 0,
    /// First fragment of a sequence.
    First = 1,
    /// Middle fragment (more to follow).
    Middle = 2,
    /// Last fragment of a sequence.
    Last = 3,
}

impl FragFlags {
    pub fn from_u8(val: u8) -> Result<Self> {
        match val & 0x03 {
            0 => Ok(FragFlags::Complete),
            1 => Ok(FragFlags::First),
            2 => Ok(FragFlags::Middle),
            3 => Ok(FragFlags::Last),
            _ => unreachable!(),
        }
    }

    /// Returns true if this is a fragmented PDU (not complete).
    pub fn is_fragmented(&self) -> bool {
        *self != FragFlags::Complete
    }
}

/// Application Protocol Data Unit.
#[derive(Debug, Clone)]
pub struct AppPdu {
    /// Application protocol identifier (4 bits).
    pub protocol: AppProtocol,
    /// Compression method (2 bits).
    pub compression: Compression,
    /// Fragmentation flags (2 bits).
    pub frag_flags: FragFlags,
    /// Fragment identifier (groups fragments of the same original message).
    pub frag_id: u8,
    /// Zero-based fragment index within the sequence (0 = first fragment).
    /// Only meaningful when `frag_flags` is not `Complete`.
    pub frag_index: u8,
    /// Total number of fragments in the sequence.
    /// Only meaningful when `frag_flags` is not `Complete`.
    pub frag_total: u8,
    /// Application payload.
    pub payload: Vec<u8>,
}

/// Application PDU header size:
///   1 (proto+compress+frag_flags) + 1 (frag_id) + 1 (frag_index) + 1 (frag_total) = 4 bytes.
pub const APP_HEADER_SIZE: usize = 4;

impl AppPdu {
    /// Create a complete (unfragmented) text message.
    pub fn new_text(text: &[u8]) -> Self {
        Self {
            protocol: AppProtocol::Text,
            compression: Compression::None,
            frag_flags: FragFlags::Complete,
            frag_id: 0,
            frag_index: 0,
            frag_total: 1,
            payload: text.to_vec(),
        }
    }

    /// Create a complete PDU with the specified protocol.
    pub fn new(protocol: AppProtocol, payload: Vec<u8>) -> Self {
        Self {
            protocol,
            compression: Compression::None,
            frag_flags: FragFlags::Complete,
            frag_id: 0,
            frag_index: 0,
            frag_total: 1,
            payload,
        }
    }

    /// Create a fragment of a larger message.
    pub fn new_fragment(
        protocol: AppProtocol,
        compression: Compression,
        frag_flags: FragFlags,
        frag_id: u8,
        frag_index: u8,
        frag_total: u8,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            protocol,
            compression,
            frag_flags,
            frag_id,
            frag_index,
            frag_total,
            payload,
        }
    }

    /// Set compression method.
    pub fn with_compression(mut self, compression: Compression) -> Self {
        self.compression = compression;
        self
    }

    /// Fragment a payload into multiple AppPDUs of at most `max_fragment_size` bytes each.
    ///
    /// Returns a vector of AppPDUs. If the payload fits in a single fragment,
    /// returns a single Complete PDU. Otherwise, returns First + Middle* + Last.
    pub fn fragment(
        protocol: AppProtocol,
        compression: Compression,
        frag_id: u8,
        payload: &[u8],
        max_fragment_size: usize,
    ) -> Vec<Self> {
        if max_fragment_size == 0 {
            return vec![Self::new_fragment(
                protocol,
                compression,
                FragFlags::Complete,
                frag_id,
                0,
                1,
                payload.to_vec(),
            )];
        }

        if payload.len() <= max_fragment_size {
            return vec![Self::new_fragment(
                protocol,
                compression,
                FragFlags::Complete,
                frag_id,
                0,
                1,
                payload.to_vec(),
            )];
        }

        let chunks: Vec<&[u8]> = payload.chunks(max_fragment_size).collect();
        let total = chunks.len() as u8;
        let last_idx = chunks.len() - 1;

        chunks
            .into_iter()
            .enumerate()
            .map(|(i, chunk)| {
                let flags = if i == 0 {
                    FragFlags::First
                } else if i == last_idx {
                    FragFlags::Last
                } else {
                    FragFlags::Middle
                };
                Self::new_fragment(
                    protocol,
                    compression,
                    flags,
                    frag_id,
                    i as u8,
                    total,
                    chunk.to_vec(),
                )
            })
            .collect()
    }

    /// Reassemble fragments into a single payload.
    ///
    /// `fragments` must be ordered: First, Middle*, Last (or a single Complete).
    /// All must share the same `frag_id`.
    pub fn reassemble(fragments: &[Self]) -> Result<Vec<u8>> {
        if fragments.is_empty() {
            return Err(anyhow!("No fragments to reassemble"));
        }

        // Single complete PDU
        if fragments.len() == 1 && fragments[0].frag_flags == FragFlags::Complete {
            return Ok(fragments[0].payload.clone());
        }

        // Validate ordering
        if fragments.first().map(|f| f.frag_flags) != Some(FragFlags::First) {
            return Err(anyhow!("First fragment must have FragFlags::First"));
        }
        if fragments.last().map(|f| f.frag_flags) != Some(FragFlags::Last) {
            return Err(anyhow!("Last fragment must have FragFlags::Last"));
        }

        let frag_id = fragments[0].frag_id;
        let expected_total = fragments[0].frag_total;
        let mut payload = Vec::new();

        if expected_total as usize != fragments.len() {
            return Err(anyhow!(
                "Fragment count mismatch: frag_total={} but got {} fragments",
                expected_total,
                fragments.len()
            ));
        }

        for (i, frag) in fragments.iter().enumerate() {
            if frag.frag_id != frag_id {
                return Err(anyhow!(
                    "Fragment ID mismatch at index {}: expected {}, got {}",
                    i,
                    frag_id,
                    frag.frag_id
                ));
            }
            if frag.frag_index != i as u8 {
                return Err(anyhow!(
                    "Fragment index mismatch at position {}: expected {}, got {}",
                    i,
                    i,
                    frag.frag_index
                ));
            }
            if frag.frag_total != expected_total {
                return Err(anyhow!(
                    "Fragment total mismatch at index {}: expected {}, got {}",
                    i,
                    expected_total,
                    frag.frag_total
                ));
            }
            let expected_flags = if i == 0 {
                FragFlags::First
            } else if i == fragments.len() - 1 {
                FragFlags::Last
            } else {
                FragFlags::Middle
            };
            if frag.frag_flags != expected_flags {
                return Err(anyhow!(
                    "Wrong frag flags at index {}: expected {:?}, got {:?}",
                    i,
                    expected_flags,
                    frag.frag_flags
                ));
            }
            payload.extend_from_slice(&frag.payload);
        }

        Ok(payload)
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(APP_HEADER_SIZE + self.payload.len());

        // Byte 0: protocol (high 4b) | compression (2b) | frag_flags (2b)
        let byte0 = ((self.protocol as u8 & 0x0F) << 4)
            | ((self.compression as u8 & 0x03) << 2)
            | (self.frag_flags as u8 & 0x03);
        out.push(byte0);

        // Byte 1: fragment ID
        out.push(self.frag_id);

        // Byte 2: fragment index (zero-based position in the sequence)
        out.push(self.frag_index);

        // Byte 3: fragment total count
        out.push(self.frag_total);

        // Payload
        out.extend_from_slice(&self.payload);

        out
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < APP_HEADER_SIZE {
            return Err(anyhow!(
                "App PDU too short: {} bytes (min {})",
                bytes.len(),
                APP_HEADER_SIZE
            ));
        }

        let protocol = AppProtocol::from_u8((bytes[0] >> 4) & 0x0F)?;
        let compression = Compression::from_u8((bytes[0] >> 2) & 0x03)?;
        let frag_flags = FragFlags::from_u8(bytes[0] & 0x03)?;
        let frag_id = bytes[1];
        let frag_index = bytes[2];
        let frag_total = bytes[3];
        let payload = bytes[APP_HEADER_SIZE..].to_vec();

        Ok(Self {
            protocol,
            compression,
            frag_flags,
            frag_id,
            frag_index,
            frag_total,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basic roundtrip tests ───────────────────────────────────────

    #[test]
    fn test_text_roundtrip() {
        let pdu = AppPdu::new_text(b"Hello, World!");
        let bytes = pdu.to_bytes();
        let decoded = AppPdu::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.protocol, AppProtocol::Text);
        assert_eq!(decoded.compression, Compression::None);
        assert_eq!(decoded.frag_flags, FragFlags::Complete);
        assert_eq!(decoded.frag_id, 0);
        assert_eq!(decoded.payload, b"Hello, World!");
    }

    #[test]
    fn test_all_protocols() {
        let protos = [
            AppProtocol::Text,
            AppProtocol::Position,
            AppProtocol::Telemetry,
            AppProtocol::FileTransfer,
            AppProtocol::Voice,
            AppProtocol::Control,
            AppProtocol::Custom,
            AppProtocol::CompressedData,
        ];
        for proto in &protos {
            let pdu = AppPdu::new(*proto, vec![0x42]);
            let bytes = pdu.to_bytes();
            let decoded = AppPdu::from_bytes(&bytes).unwrap();
            assert_eq!(decoded.protocol, *proto);
        }
    }

    #[test]
    fn test_invalid_protocol() {
        assert!(AppProtocol::from_u8(8).is_err());
        assert!(AppProtocol::from_u8(15).is_err());
    }

    #[test]
    fn test_all_compression_types() {
        for comp in [
            Compression::None,
            Compression::Deflate,
            Compression::Lz4,
            Compression::Reserved,
        ] {
            let pdu = AppPdu::new(AppProtocol::Text, vec![]).with_compression(comp);
            let bytes = pdu.to_bytes();
            let decoded = AppPdu::from_bytes(&bytes).unwrap();
            assert_eq!(decoded.compression, comp);
        }
    }

    #[test]
    fn test_all_frag_flags() {
        let flags = [
            FragFlags::Complete,
            FragFlags::First,
            FragFlags::Middle,
            FragFlags::Last,
        ];
        for &flag in &flags {
            let val = flag as u8;
            let decoded = FragFlags::from_u8(val).unwrap();
            assert_eq!(decoded, flag);
        }
    }

    #[test]
    fn test_is_fragmented() {
        assert!(!FragFlags::Complete.is_fragmented());
        assert!(FragFlags::First.is_fragmented());
        assert!(FragFlags::Middle.is_fragmented());
        assert!(FragFlags::Last.is_fragmented());
    }

    #[test]
    fn test_empty_payload() {
        let pdu = AppPdu::new_text(b"");
        let bytes = pdu.to_bytes();
        assert_eq!(bytes.len(), APP_HEADER_SIZE);
        let decoded = AppPdu::from_bytes(&bytes).unwrap();
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn test_too_short() {
        assert!(AppPdu::from_bytes(&[]).is_err());
        assert!(AppPdu::from_bytes(&[0x00]).is_err());
    }

    #[test]
    fn test_header_byte_encoding() {
        let pdu = AppPdu::new_fragment(
            AppProtocol::FileTransfer, // 3
            Compression::Lz4,          // 2
            FragFlags::Middle,         // 2
            42,
            1, // frag_index
            3, // frag_total
            vec![],
        );
        let bytes = pdu.to_bytes();
        // Byte 0: (3 << 4) | (2 << 2) | 2 = 0x30 | 0x08 | 0x02 = 0x3A
        assert_eq!(bytes[0], 0x3A);
        assert_eq!(bytes[1], 42);
        assert_eq!(bytes[2], 1); // frag_index
        assert_eq!(bytes[3], 3); // frag_total
    }

    // ── Fragmentation tests ─────────────────────────────────────────

    #[test]
    fn test_fragment_small_payload() {
        // Payload fits in one fragment
        let fragments = AppPdu::fragment(AppProtocol::Text, Compression::None, 1, b"Small", 100);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].frag_flags, FragFlags::Complete);
        assert_eq!(fragments[0].payload, b"Small");
    }

    #[test]
    fn test_fragment_exact_fit() {
        let data = vec![0xAA; 50];
        let fragments = AppPdu::fragment(AppProtocol::Text, Compression::None, 2, &data, 50);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].frag_flags, FragFlags::Complete);
    }

    #[test]
    fn test_fragment_two_parts() {
        let data = vec![0xBB; 100];
        let fragments = AppPdu::fragment(AppProtocol::Text, Compression::None, 3, &data, 60);
        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].frag_flags, FragFlags::First);
        assert_eq!(fragments[0].payload.len(), 60);
        assert_eq!(fragments[1].frag_flags, FragFlags::Last);
        assert_eq!(fragments[1].payload.len(), 40);
    }

    #[test]
    fn test_fragment_three_parts() {
        let data = vec![0xCC; 150];
        let fragments = AppPdu::fragment(AppProtocol::Text, Compression::None, 4, &data, 60);
        assert_eq!(fragments.len(), 3);
        assert_eq!(fragments[0].frag_flags, FragFlags::First);
        assert_eq!(fragments[1].frag_flags, FragFlags::Middle);
        assert_eq!(fragments[2].frag_flags, FragFlags::Last);

        // All should share the same frag_id
        for f in &fragments {
            assert_eq!(f.frag_id, 4);
        }
    }

    #[test]
    fn test_fragment_many_parts() {
        let data = vec![0xDD; 500];
        let fragments =
            AppPdu::fragment(AppProtocol::FileTransfer, Compression::None, 5, &data, 50);
        assert_eq!(fragments.len(), 10);
        assert_eq!(fragments[0].frag_flags, FragFlags::First);
        for f in &fragments[1..9] {
            assert_eq!(f.frag_flags, FragFlags::Middle);
        }
        assert_eq!(fragments[9].frag_flags, FragFlags::Last);
    }

    // ── Reassembly tests ────────────────────────────────────────────

    #[test]
    fn test_reassemble_complete() {
        let pdu = AppPdu::new_text(b"Complete");
        let result = AppPdu::reassemble(&[pdu]).unwrap();
        assert_eq!(result, b"Complete");
    }

    #[test]
    fn test_fragment_and_reassemble_roundtrip() {
        let original = b"This is a longer message that needs fragmentation to fit.";
        let fragments = AppPdu::fragment(AppProtocol::Text, Compression::None, 10, original, 20);

        assert!(fragments.len() > 1);
        let reassembled = AppPdu::reassemble(&fragments).unwrap();
        assert_eq!(reassembled, original);
    }

    #[test]
    fn test_reassemble_serialized_roundtrip() {
        let original = vec![0xEE; 200];
        let fragments = AppPdu::fragment(
            AppProtocol::Telemetry,
            Compression::Deflate,
            7,
            &original,
            30,
        );

        // Serialize and deserialize each fragment
        let deserialized: Vec<AppPdu> = fragments
            .iter()
            .map(|f| {
                let bytes = f.to_bytes();
                AppPdu::from_bytes(&bytes).unwrap()
            })
            .collect();

        let reassembled = AppPdu::reassemble(&deserialized).unwrap();
        assert_eq!(reassembled, original);
    }

    #[test]
    fn test_reassemble_empty() {
        assert!(AppPdu::reassemble(&[]).is_err());
    }

    #[test]
    fn test_reassemble_wrong_first() {
        let frag = AppPdu::new_fragment(
            AppProtocol::Text,
            Compression::None,
            FragFlags::Middle,
            1,
            0,
            1,
            vec![],
        );
        assert!(AppPdu::reassemble(&[frag]).is_err());
    }

    #[test]
    fn test_reassemble_wrong_last() {
        let f1 = AppPdu::new_fragment(
            AppProtocol::Text,
            Compression::None,
            FragFlags::First,
            1,
            0,
            2,
            vec![0x01],
        );
        let f2 = AppPdu::new_fragment(
            AppProtocol::Text,
            Compression::None,
            FragFlags::Middle,
            1,
            1,
            2,
            vec![0x02],
        );
        assert!(AppPdu::reassemble(&[f1, f2]).is_err());
    }

    #[test]
    fn test_reassemble_mismatched_frag_id() {
        let f1 = AppPdu::new_fragment(
            AppProtocol::Text,
            Compression::None,
            FragFlags::First,
            1,
            0,
            2,
            vec![0x01],
        );
        let f2 = AppPdu::new_fragment(
            AppProtocol::Text,
            Compression::None,
            FragFlags::Last,
            2, // different frag_id
            1,
            2,
            vec![0x02],
        );
        assert!(AppPdu::reassemble(&[f1, f2]).is_err());
    }

    // ── Full PDU stack test ─────────────────────────────────────────

    #[test]
    fn test_large_payload() {
        let payload = vec![0xFF; 1000];
        let pdu = AppPdu::new(AppProtocol::FileTransfer, payload.clone());
        let bytes = pdu.to_bytes();
        let decoded = AppPdu::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.payload, payload);
    }
}
