//! Repeated, unacknowledged file chunks over application PDUs.
//! See docs/SPEC.md for the wire format. CRC detects damage, not forgery.

use crate::app::{AppPdu, AppProtocol};
use crc::{Crc, CRC_64_ECMA_182};

const LIMIT: usize = 1_048_576;
const CHUNK: usize = 384;
const CRC: Crc<u64> = Crc::<u64>::new(&CRC_64_ECMA_182);

/// A broadcast packet or file failed validation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BroadcastError {
    /// The complete file exceeds the bounded receiver capacity.
    #[error("broadcast file exceeds 1048576 bytes")]
    Oversize,
    /// A file packet has invalid metadata or framing.
    #[error("malformed broadcast file packet")]
    Malformed,
    /// A previously accepted index arrived with different bytes.
    #[error("conflicting broadcast file chunk")]
    Conflict,
    /// All chunks arrived but do not match the content checksum.
    #[error("broadcast file checksum mismatch")]
    Checksum,
}

/// Encode complete AppPdu packets, suitable for individual KISS datagrams.
pub fn encode_file(bytes: &[u8]) -> Result<Vec<Vec<u8>>, BroadcastError> {
    if bytes.len() > LIMIT {
        return Err(BroadcastError::Oversize);
    }
    let count = bytes.len().div_ceil(CHUNK).max(1);
    let checksum = CRC.checksum(bytes);
    let mut packets = Vec::with_capacity(count);
    for index in 0..count {
        let start = index * CHUNK;
        let end = (start + CHUNK).min(bytes.len());
        let mut payload = b"CPBF".to_vec();
        payload.push(1);
        payload.extend_from_slice(&checksum.to_be_bytes());
        payload.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        payload.extend_from_slice(&(index as u16).to_be_bytes());
        payload.extend_from_slice(&(count as u16).to_be_bytes());
        payload.extend_from_slice(&bytes[start..end]);
        packets.push(AppPdu::new(AppProtocol::FileTransfer, payload).to_bytes());
    }
    Ok(packets)
}

#[derive(Debug)]
struct Transfer {
    crc: u64,
    bytes: Vec<u8>,
    present: Vec<bool>,
    remaining: usize,
}

/// Accumulates the first valid file heard, across unordered repeated copies.
#[derive(Debug, Default)]
pub struct FileReceiver {
    transfer: Option<Transfer>,
    completed: bool,
    accepted: usize,
}

impl FileReceiver {
    /// Start with no selected transfer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct chunks accepted. Useful for an idle deadline.
    pub fn accepted_chunks(&self) -> usize {
        self.accepted
    }

    /// Accept a packet. Emits a verified file once, and never acknowledges it.
    /// Invalid packets and conflicting duplicates leave retained data unchanged.
    pub fn push(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>, BroadcastError> {
        if self.completed || packet.first().map(|b| b >> 4) != Some(3) {
            return Ok(None);
        }
        // Check the canonical AppPdu wrapper before copying any peer data.
        if packet.len() < 25 || packet.len() > 409 || packet[..4] != [0x30, 0, 0, 1] {
            return Err(BroadcastError::Malformed);
        }
        let p = &packet[4..];
        if &p[..4] != b"CPBF" || p[4] != 1 {
            return Err(BroadcastError::Malformed);
        }
        let crc = u64::from_be_bytes(p[5..13].try_into().map_err(|_| BroadcastError::Malformed)?);
        let len = u32::from_be_bytes(
            p[13..17]
                .try_into()
                .map_err(|_| BroadcastError::Malformed)?,
        ) as usize;
        let index = u16::from_be_bytes([p[17], p[18]]) as usize;
        let count = u16::from_be_bytes([p[19], p[20]]) as usize;
        if len > LIMIT {
            return Err(BroadcastError::Oversize);
        }
        if count != len.div_ceil(CHUNK).max(1) || index >= count {
            return Err(BroadcastError::Malformed);
        }
        let start = index * CHUNK;
        let end = (start + CHUNK).min(len);
        let data = &p[21..];
        if data.len() != end - start {
            return Err(BroadcastError::Malformed);
        }
        if let Some(t) = &self.transfer {
            if t.crc != crc {
                return Ok(None);
            }
            if t.bytes.len() != len {
                return Err(BroadcastError::Malformed);
            }
        }
        // A rejected complete packet must not select an identity and prevent
        // a later valid transfer from being heard.
        if self.transfer.is_none() && count == 1 && CRC.checksum(data) != crc {
            return Err(BroadcastError::Checksum);
        }
        let t = self.transfer.get_or_insert_with(|| Transfer {
            crc,
            bytes: vec![0; len],
            present: vec![false; count],
            remaining: count,
        });
        if t.present[index] {
            return if &t.bytes[start..end] == data {
                Ok(None)
            } else {
                Err(BroadcastError::Conflict)
            };
        }
        // Validate completion before mutating retained state. A damaged final
        // chunk can be replaced by the next copy.
        if t.remaining == 1 {
            let mut digest = CRC.digest();
            digest.update(&t.bytes[..start]);
            digest.update(data);
            digest.update(&t.bytes[end..]);
            if digest.finalize() != crc {
                return Err(BroadcastError::Checksum);
            }
        }
        t.bytes[start..end].copy_from_slice(data);
        t.present[index] = true;
        t.remaining -= 1;
        self.accepted += 1;
        if t.remaining == 0 {
            self.completed = true;
            return Ok(self.transfer.take().map(|t| t.bytes));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_empty_binary_maximum_and_packet_size() {
        for len in [0, 5, 384, 1000, LIMIT] {
            let bytes: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let packets = encode_file(&bytes).unwrap();
            assert!(packets.iter().all(|p| p.len() <= 409));
            if len >= 384 {
                assert_eq!(packets[0].len(), 409);
            }
            let mut rx = FileReceiver::new();
            let mut result = None;
            for p in packets.iter().rev() {
                result = rx.push(p).unwrap().or(result);
            }
            assert_eq!(result, Some(bytes));
            assert_eq!(rx.push(&packets[0]).unwrap(), None);
        }
        assert_eq!(
            encode_file(&vec![0; LIMIT + 1]),
            Err(BroadcastError::Oversize)
        );
    }

    #[test]
    fn fixed_vector() {
        assert_eq!(
            encode_file(b"123456789").unwrap()[0],
            [
                vec![
                    0x30, 0, 0, 1, b'C', b'P', b'B', b'F', 1, 0x6c, 0x40, 0xdf, 0x5f, 0x0b, 0x49,
                    0x73, 0x47, 0, 0, 0, 9, 0, 0, 0, 1
                ],
                b"123456789".to_vec()
            ]
            .concat()
        );
    }

    #[test]
    fn late_join_duplicates_and_interleaved_identity() {
        let bytes = vec![0xdb; 1000];
        let packets = encode_file(&bytes).unwrap();
        let mut rx = FileReceiver::new();
        assert_eq!(rx.push(&packets[1]).unwrap(), None);
        assert_eq!(rx.push(&packets[1]).unwrap(), None);
        assert_eq!(rx.push(&encode_file(b"other").unwrap()[0]).unwrap(), None);
        assert_eq!(rx.push(&packets[2]).unwrap(), None);
        assert_eq!(rx.push(&packets[0]).unwrap(), Some(bytes));
    }

    #[test]
    fn malformed_packets_do_not_pin_state() {
        let p = encode_file(b"x").unwrap().remove(0);
        for (offset, value) in [(0, 0x34), (3, 2), (8, 2), (20, 2), (22, 1), (24, 2)] {
            let mut bad = p.clone();
            bad[offset] = value;
            let mut rx = FileReceiver::new();
            assert!(rx.push(&bad).is_err(), "offset {offset}");
            assert_eq!(rx.push(&p).unwrap(), Some(b"x".to_vec()));
        }
        for len in 1..p.len() {
            assert!(FileReceiver::new().push(&p[..len]).is_err());
        }
        let mut bad = p.clone();
        bad.push(0);
        assert!(FileReceiver::new().push(&bad).is_err());
    }

    #[test]
    fn invalid_single_chunk_does_not_select_transfer() {
        let mut bad = encode_file(b"bad").unwrap().remove(0);
        bad[25] ^= 1;
        let mut rx = FileReceiver::new();
        assert_eq!(rx.push(&bad), Err(BroadcastError::Checksum));
        assert_eq!(rx.accepted_chunks(), 0);
        assert_eq!(
            rx.push(&encode_file(b"valid").unwrap()[0]).unwrap(),
            Some(b"valid".to_vec())
        );
    }

    #[test]
    fn conflicts_and_failed_checksum_preserve_state() {
        let bytes = vec![7; 800];
        let p = encode_file(&bytes).unwrap();
        let mut rx = FileReceiver::new();
        rx.push(&p[0]).unwrap();
        let mut bad = p[0].clone();
        bad[25] ^= 1;
        assert_eq!(rx.push(&bad), Err(BroadcastError::Conflict));
        rx.push(&p[1]).unwrap();
        let mut bad = p[2].clone();
        bad[25] ^= 1;
        assert_eq!(rx.push(&bad), Err(BroadcastError::Checksum));
        assert_eq!(rx.push(&p[2]).unwrap(), Some(bytes));
    }
}
