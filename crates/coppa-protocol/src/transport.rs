//! Transport layer Protocol Data Unit for Coppa.
//!
//! ```text
//! Transport PDU → [SessionID 4b][Type 4b][SeqNum 1B][AckNum 1B][AckBitmap 1B][App PDU]
//! ```
//!
//! The transport layer provides reliable, ordered delivery over the MAC layer
//! using sequence numbers and selective acknowledgment bitmaps.

use anyhow::{anyhow, Result};

/// Transport PDU types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TransportType {
    /// Unreliable datagram (no ACK expected).
    Unreliable = 0,
    /// Reliable data segment (requires ACK).
    Reliable = 1,
    /// Standalone ACK (no application payload).
    Ack = 2,
    /// Negative ACK / retransmit request.
    Nak = 3,
    /// Reset the transport session.
    Reset = 4,
}

impl TransportType {
    pub fn from_u8(val: u8) -> Result<Self> {
        match val & 0x0F {
            0 => Ok(TransportType::Unreliable),
            1 => Ok(TransportType::Reliable),
            2 => Ok(TransportType::Ack),
            3 => Ok(TransportType::Nak),
            4 => Ok(TransportType::Reset),
            v => Err(anyhow!("Unknown transport type: {}", v)),
        }
    }
}

/// Transport Protocol Data Unit.
///
/// Provides session multiplexing, sequencing, and selective acknowledgment.
#[derive(Debug, Clone)]
pub struct TransportPdu {
    /// Session ID (4 bits, 0-15).
    pub session_id: u8,
    /// Transport segment type.
    pub transport_type: TransportType,
    /// Sequence number for this segment (0-255, wrapping).
    pub seq_num: u8,
    /// Cumulative acknowledgment number: all segments up to (but not including)
    /// this number have been received.
    pub ack_num: u8,
    /// Selective ACK bitmap: bit N set means segment (ack_num + N + 1) received.
    /// Covers 8 segments beyond the cumulative ACK.
    pub ack_bitmap: u8,
    /// Application PDU payload.
    pub payload: Vec<u8>,
}

/// Transport PDU header size: 1 (session_id+type) + 1 (seq) + 1 (ack) + 1 (bitmap) = 4 bytes.
pub const TRANSPORT_HEADER_SIZE: usize = 4;

impl TransportPdu {
    /// Create a new reliable data segment.
    pub fn new_reliable(session_id: u8, seq_num: u8, ack_num: u8, payload: Vec<u8>) -> Self {
        Self {
            session_id: session_id & 0x0F,
            transport_type: TransportType::Reliable,
            seq_num,
            ack_num,
            ack_bitmap: 0,
            payload,
        }
    }

    /// Create a new unreliable datagram.
    pub fn new_unreliable(session_id: u8, seq_num: u8, payload: Vec<u8>) -> Self {
        Self {
            session_id: session_id & 0x0F,
            transport_type: TransportType::Unreliable,
            seq_num,
            ack_num: 0,
            ack_bitmap: 0,
            payload,
        }
    }

    /// Create a standalone ACK.
    pub fn new_ack(session_id: u8, ack_num: u8, ack_bitmap: u8) -> Self {
        Self {
            session_id: session_id & 0x0F,
            transport_type: TransportType::Ack,
            seq_num: 0,
            ack_num,
            ack_bitmap,
            payload: Vec::new(),
        }
    }

    /// Create a NAK (negative acknowledgment / retransmit request).
    pub fn new_nak(session_id: u8, ack_num: u8, ack_bitmap: u8) -> Self {
        Self {
            session_id: session_id & 0x0F,
            transport_type: TransportType::Nak,
            seq_num: 0,
            ack_num,
            ack_bitmap,
            payload: Vec::new(),
        }
    }

    /// Create a transport reset.
    pub fn new_reset(session_id: u8) -> Self {
        Self {
            session_id: session_id & 0x0F,
            transport_type: TransportType::Reset,
            seq_num: 0,
            ack_num: 0,
            ack_bitmap: 0,
            payload: Vec::new(),
        }
    }

    /// Check if a specific sequence number is selectively acknowledged.
    ///
    /// Returns true if `seq` is either cumulatively acknowledged (seq < ack_num)
    /// or selectively acknowledged via the bitmap.
    pub fn is_acked(&self, seq: u8) -> bool {
        // Cumulative ACK: ack_num acknowledges up to but NOT including ack_num.
        let diff = seq.wrapping_sub(self.ack_num);
        if diff == 0 {
            // seq == ack_num: NOT cumulatively acked
            return false;
        }
        if diff > 128 {
            // seq < ack_num (considering wrapping)
            return true;
        }
        // Selective ACK bitmap: bit 0 = ack_num+1, bit 1 = ack_num+2, etc.
        if diff <= 8 {
            let bit_index = diff - 1;
            return (self.ack_bitmap >> bit_index) & 1 == 1;
        }
        false
    }

    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(TRANSPORT_HEADER_SIZE + self.payload.len());

        // Byte 0: session_id (high nibble) | type (low nibble)
        out.push(((self.session_id & 0x0F) << 4) | (self.transport_type as u8 & 0x0F));

        // Byte 1: sequence number
        out.push(self.seq_num);

        // Byte 2: acknowledgment number
        out.push(self.ack_num);

        // Byte 3: selective ACK bitmap
        out.push(self.ack_bitmap);

        // Application payload
        out.extend_from_slice(&self.payload);

        out
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < TRANSPORT_HEADER_SIZE {
            return Err(anyhow!(
                "Transport PDU too short: {} bytes (min {})",
                bytes.len(),
                TRANSPORT_HEADER_SIZE
            ));
        }

        let session_id = (bytes[0] >> 4) & 0x0F;
        let transport_type = TransportType::from_u8(bytes[0] & 0x0F)?;
        let seq_num = bytes[1];
        let ack_num = bytes[2];
        let ack_bitmap = bytes[3];
        let payload = bytes[TRANSPORT_HEADER_SIZE..].to_vec();

        Ok(Self {
            session_id,
            transport_type,
            seq_num,
            ack_num,
            ack_bitmap,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reliable_roundtrip() {
        let payload = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        let pdu = TransportPdu::new_reliable(3, 42, 10, payload.clone());

        let bytes = pdu.to_bytes();
        let decoded = TransportPdu::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.session_id, 3);
        assert_eq!(decoded.transport_type, TransportType::Reliable);
        assert_eq!(decoded.seq_num, 42);
        assert_eq!(decoded.ack_num, 10);
        assert_eq!(decoded.ack_bitmap, 0);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn test_unreliable_roundtrip() {
        let pdu = TransportPdu::new_unreliable(7, 100, vec![0xFF]);
        let bytes = pdu.to_bytes();
        let decoded = TransportPdu::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.session_id, 7);
        assert_eq!(decoded.transport_type, TransportType::Unreliable);
        assert_eq!(decoded.seq_num, 100);
        assert_eq!(decoded.payload, vec![0xFF]);
    }

    #[test]
    fn test_ack_roundtrip() {
        let pdu = TransportPdu::new_ack(1, 50, 0b10101010);
        let bytes = pdu.to_bytes();
        let decoded = TransportPdu::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.session_id, 1);
        assert_eq!(decoded.transport_type, TransportType::Ack);
        assert_eq!(decoded.ack_num, 50);
        assert_eq!(decoded.ack_bitmap, 0b10101010);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn test_nak_roundtrip() {
        let pdu = TransportPdu::new_nak(2, 25, 0b00001111);
        let bytes = pdu.to_bytes();
        let decoded = TransportPdu::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.transport_type, TransportType::Nak);
        assert_eq!(decoded.ack_num, 25);
        assert_eq!(decoded.ack_bitmap, 0b00001111);
    }

    #[test]
    fn test_reset_roundtrip() {
        let pdu = TransportPdu::new_reset(15);
        let bytes = pdu.to_bytes();
        let decoded = TransportPdu::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.session_id, 15);
        assert_eq!(decoded.transport_type, TransportType::Reset);
    }

    #[test]
    fn test_all_transport_types() {
        let types = [
            TransportType::Unreliable,
            TransportType::Reliable,
            TransportType::Ack,
            TransportType::Nak,
            TransportType::Reset,
        ];
        for &tt in &types {
            let val = tt as u8;
            let decoded = TransportType::from_u8(val).unwrap();
            assert_eq!(decoded, tt);
        }
    }

    #[test]
    fn test_invalid_transport_type() {
        assert!(TransportType::from_u8(5).is_err());
        assert!(TransportType::from_u8(15).is_err());
    }

    #[test]
    fn test_too_short() {
        assert!(TransportPdu::from_bytes(&[0x00]).is_err());
        assert!(TransportPdu::from_bytes(&[0x00, 0x01, 0x02]).is_err());
    }

    #[test]
    fn test_session_id_masking() {
        let pdu = TransportPdu::new_reliable(0xFF, 0, 0, vec![]);
        assert_eq!(pdu.session_id, 0x0F);
    }

    #[test]
    fn test_header_size() {
        let pdu = TransportPdu::new_reliable(0, 0, 0, vec![]);
        let bytes = pdu.to_bytes();
        assert_eq!(bytes.len(), TRANSPORT_HEADER_SIZE);
    }

    #[test]
    fn test_empty_payload() {
        let pdu = TransportPdu::new_reliable(5, 10, 5, vec![]);
        let bytes = pdu.to_bytes();
        let decoded = TransportPdu::from_bytes(&bytes).unwrap();
        assert!(decoded.payload.is_empty());
    }

    // ── Selective ACK tests ─────────────────────────────────────────

    #[test]
    fn test_is_acked_cumulative() {
        let pdu = TransportPdu::new_ack(0, 10, 0);
        // ack_num acknowledges up to but NOT including ack_num
        assert!(pdu.is_acked(0));
        assert!(pdu.is_acked(5));
        assert!(pdu.is_acked(9));
        assert!(!pdu.is_acked(10)); // ack_num itself is NOT acked
        assert!(!pdu.is_acked(11));
        assert!(!pdu.is_acked(18));
    }

    #[test]
    fn test_is_acked_selective() {
        // bitmap = 0b00001010: bits 1 and 3 set
        // -> ack_num+2 and ack_num+4 are selectively ACKed
        let pdu = TransportPdu::new_ack(0, 10, 0b00001010);
        assert!(pdu.is_acked(9)); // cumulative
        assert!(!pdu.is_acked(11)); // ack_num+1, bit 0 = 0
        assert!(pdu.is_acked(12)); // ack_num+2, bit 1 = 1
        assert!(!pdu.is_acked(13)); // ack_num+3, bit 2 = 0
        assert!(pdu.is_acked(14)); // ack_num+4, bit 3 = 1
        assert!(!pdu.is_acked(15)); // ack_num+5, bit 4 = 0
        assert!(!pdu.is_acked(19)); // beyond bitmap range
    }

    #[test]
    fn test_is_acked_wrapping() {
        // Test near wrapping boundary
        let pdu = TransportPdu::new_ack(0, 254, 0b00000001);
        assert!(pdu.is_acked(253)); // cumulative (< ack_num)
        assert!(!pdu.is_acked(254)); // ack_num itself is NOT acked
        assert!(pdu.is_acked(255)); // selective bit 0
        assert!(!pdu.is_acked(0)); // beyond selective (wraps)
    }

    #[test]
    fn test_seq_wrapping() {
        // Sequence numbers should wrap at 255
        let pdu = TransportPdu::new_reliable(0, 255, 0, vec![]);
        let bytes = pdu.to_bytes();
        let decoded = TransportPdu::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.seq_num, 255);
    }

    #[test]
    fn test_large_payload() {
        let payload = vec![0xCC; 200];
        let pdu = TransportPdu::new_reliable(0, 0, 0, payload.clone());
        let bytes = pdu.to_bytes();
        let decoded = TransportPdu::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.payload, payload);
    }
}
