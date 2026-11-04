//! Packetization and bounded reassembly for session fade protection, SPEC §22.
use crate::{
    fec::outer_rs::OuterRs12x6, mac::MacPdu, modem::speed_levels::max_payload_for_level,
    ProtocolError,
};
/// Number of source shards.
pub const OUTER_K: usize = 6;
/// Number of transmitted shards.
pub const OUTER_N: usize = 12;
/// Outer header size in bytes.
pub const OUTER_HEADER_LEN: usize = 4;
/// RS(12,6) wire format nibble.
pub const OUTER_FORMAT_RS12_6: u8 = 1;
/// Shard capacity of one single-codeword PHY frame.
pub fn shard_len_for_level(level: u8) -> Option<usize> {
    max_payload_for_level(level)?.checked_sub(MacPdu::HEADER_SIZE + OUTER_HEADER_LEN)
}
/// Maximum encoded transport segment size at this level.
pub fn max_segment_len(level: u8) -> Option<usize> {
    shard_len_for_level(level)?.checked_mul(OUTER_K)
}
/// PHY HARQ key. Callers supply a validated packet index, 0 through 11.
pub fn harq_key(seq: u8, index: u8) -> u8 {
    0x10 + (seq % 20) * OUTER_N as u8 + (index % OUTER_N as u8)
}
/// One CRC-valid packet from an outer-coded segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OuterPacket {
    /// Shard index, 0 through 11.
    pub index: u8,
    /// ARQ sequence number.
    pub seq: u8,
    /// Unpadded transport segment length.
    pub segment_len: u16,
    /// Fixed-length shard bytes.
    pub shard: Vec<u8>,
}
impl OuterPacket {
    fn valid(&self) -> bool {
        (self.index as usize) < OUTER_N
            && self.segment_len > 0
            && !self.shard.is_empty()
            && self.shard.len() <= u16::MAX as usize
            && self.segment_len as usize <= self.shard.len() * OUTER_K
    }
    /// Serialize after validating all lengths and the index.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        if !self.valid() {
            return Err(ProtocolError::MalformedOuterPacket);
        }
        let mut bytes = Vec::with_capacity(OUTER_HEADER_LEN + self.shard.len());
        bytes.push((OUTER_FORMAT_RS12_6 << 4) | self.index);
        bytes.push(self.seq);
        bytes.extend_from_slice(&self.segment_len.to_be_bytes());
        bytes.extend_from_slice(&self.shard);
        Ok(bytes)
    }
    /// Parse an independently CRC-validated packet, rejecting malformed metadata.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() <= OUTER_HEADER_LEN || bytes[0] >> 4 != OUTER_FORMAT_RS12_6 {
            return Err(ProtocolError::MalformedOuterPacket);
        }
        let packet = Self {
            index: bytes[0] & 15,
            seq: bytes[1],
            segment_len: u16::from_be_bytes([bytes[2], bytes[3]]),
            shard: bytes[OUTER_HEADER_LEN..].to_vec(),
        };
        if !packet.valid() {
            return Err(ProtocolError::MalformedOuterPacket);
        }
        Ok(packet)
    }
}
/// Pad and encode one transport segment into twelve independently framed shards.
pub fn encode_block(
    seq: u8,
    segment: &[u8],
    shard_len: usize,
) -> Result<Vec<OuterPacket>, ProtocolError> {
    if segment.is_empty()
        || segment.len() > u16::MAX as usize
        || shard_len == 0
        || shard_len > u16::MAX as usize
        || segment.len() > shard_len * OUTER_K
    {
        return Err(ProtocolError::MalformedOuterPacket);
    }
    let mut padded = vec![0; shard_len * OUTER_K];
    padded[..segment.len()].copy_from_slice(segment);
    let input = std::array::from_fn(|i| &padded[i * shard_len..(i + 1) * shard_len]);
    Ok(OuterRs12x6::new()
        .encode(&input)
        .into_iter()
        .enumerate()
        .map(|(i, shard)| OuterPacket {
            index: i as u8,
            seq,
            segment_len: segment.len() as u16,
            shard,
        })
        .collect())
}
/// Result of accepting one outer packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertOutcome {
    /// Valid shard retained, awaiting more.
    Stored,
    /// This shard index was already retained.
    Duplicate,
    /// Invalid packet or inconsistent metadata.
    Rejected(&'static str),
    /// Full unpadded transport segment reconstructed.
    Rebuilt(Vec<u8>),
}
/// One in-progress block; retained shards survive retransmissions.
#[derive(Debug, Default)]
pub struct OuterBlockRx {
    seq: Option<u8>,
    segment_len: usize,
    shard_len: usize,
    shards: [Option<Vec<u8>>; OUTER_N],
}
impl OuterBlockRx {
    /// Forget every retained shard.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// Whether a block is currently retained.
    pub fn holds(&self) -> bool {
        self.seq.is_some()
    }
    /// Store a validated packet and rebuild once six distinct indices are present.
    pub fn insert(&mut self, packet: &OuterPacket) -> InsertOutcome {
        if !packet.valid() {
            return InsertOutcome::Rejected("invalid packet");
        }
        if !self.holds() || self.seq != Some(packet.seq) {
            self.reset();
            self.seq = Some(packet.seq);
            self.segment_len = packet.segment_len as usize;
            self.shard_len = packet.shard.len();
        }
        if self.segment_len != packet.segment_len as usize || self.shard_len != packet.shard.len() {
            return InsertOutcome::Rejected("inconsistent block lengths");
        }
        let slot = &mut self.shards[packet.index as usize];
        if slot.is_some() {
            return InsertOutcome::Duplicate;
        }
        *slot = Some(packet.shard.clone());
        let available: Vec<_> = self
            .shards
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (i, s.as_slice())))
            .collect();
        if let Some(data) = OuterRs12x6::new().decode(&available) {
            let mut bytes: Vec<_> = data.into_iter().flatten().collect();
            bytes.truncate(self.segment_len);
            InsertOutcome::Rebuilt(bytes)
        } else {
            InsertOutcome::Stored
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_round_trip_and_rejection() {
        let packets = encode_block(255, &[42], 99).unwrap();
        for packet in &packets {
            assert_eq!(
                OuterPacket::from_bytes(&packet.to_bytes().unwrap()).unwrap(),
                *packet
            );
        }
        for bytes in [
            &[][..],
            &[0x20, 0, 0, 1, 0],
            &[0x1c, 0, 0, 1, 0],
            &[0x10, 0, 0, 0, 0],
            &[0x10, 0, 0, 7, 0],
        ] {
            assert!(OuterPacket::from_bytes(bytes).is_err());
        }
        assert_eq!(shard_len_for_level(2), Some(99));
        assert_eq!(max_segment_len(2), Some(594));
        let mut keys = [false; 256];
        for seq in 0..=255 {
            for i in 0..12 {
                let key = harq_key(seq, i);
                assert!(key >= 16);
                keys[key as usize] = true;
            }
        }
        assert!(keys[16..].iter().all(|x| *x));
    }
    #[test]
    fn every_six_packet_subset_rebuilds_short_and_full_segment() {
        for len in [1, 594] {
            let segment: Vec<u8> = (0..len).map(|i| (i * 19) as u8).collect();
            let packets = encode_block(42, &segment, 99).unwrap();
            for mask in 0u16..4096 {
                if mask.count_ones() != 6 {
                    continue;
                }
                let mut rx = OuterBlockRx::default();
                let mut rebuilt = None;
                for (i, packet) in packets.iter().enumerate() {
                    if mask & (1 << i) != 0 {
                        if let InsertOutcome::Rebuilt(bytes) = rx.insert(packet) {
                            rebuilt = Some(bytes);
                        }
                    }
                }
                assert_eq!(rebuilt.as_deref(), Some(segment.as_slice()));
            }
        }
    }
    #[test]
    fn reassembly_retains_across_attempts_resets_and_rejects_mismatch() {
        for len in [1, 594] {
            let segment: Vec<u8> = (0..len).map(|i| (i * 19) as u8).collect();
            let packets = encode_block(3, &segment, 99).unwrap();
            let mut rx = OuterBlockRx::default();
            for i in [0, 2, 4, 7, 9] {
                assert_eq!(rx.insert(&packets[i]), InsertOutcome::Stored);
            }
            assert_eq!(rx.insert(&packets[0]), InsertOutcome::Duplicate);
            let mut bad = packets[1].clone();
            bad.shard.push(0);
            assert!(matches!(rx.insert(&bad), InsertOutcome::Rejected(_)));
            assert_eq!(rx.insert(&packets[11]), InsertOutcome::Rebuilt(segment));
            let next = encode_block(4, &[1], 99).unwrap();
            assert_eq!(rx.insert(&next[0]), InsertOutcome::Stored);
            assert!(rx.holds());
            rx.reset();
            assert!(!rx.holds());
        }
    }
}
