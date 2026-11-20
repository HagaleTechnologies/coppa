//! Bounded stop-and-wait fragments for one reliable transport PDU on the floor.

use coppa_protocol::transport::{TransportPdu, TransportType};
use crc::{Crc, CRC_32_ISO_HDLC};

pub(crate) const HEADER: usize = 8;
pub(crate) const CHUNK: usize = 18;
const MAX_TRANSPORT: usize = 4103;
const CRC: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub seq: u8,
    pub len: u16,
    pub crc: u32,
}

impl Identity {
    pub fn new(data: &[u8], seq: u8) -> Option<Self> {
        if !(27..=MAX_TRANSPORT).contains(&data.len()) {
            return None;
        }
        Some(Self {
            seq,
            len: data.len() as u16,
            crc: CRC.checksum(data),
        })
    }

    pub fn parse(bytes: &[u8]) -> Option<(Self, u8, &[u8])> {
        if bytes.len() < HEADER {
            return None;
        }
        let id = Self {
            seq: bytes[0],
            len: u16::from_be_bytes([bytes[2], bytes[3]]),
            crc: u32::from_be_bytes(bytes[4..8].try_into().ok()?),
        };
        if !(27..=MAX_TRANSPORT).contains(&(id.len as usize)) {
            return None;
        }
        Some((id, bytes[1], &bytes[HEADER..]))
    }

    pub fn header(self, index: u8) -> Vec<u8> {
        let mut out = vec![self.seq, index];
        out.extend_from_slice(&self.len.to_be_bytes());
        out.extend_from_slice(&self.crc.to_be_bytes());
        out
    }

    pub fn count(self) -> usize {
        (self.len as usize).div_ceil(CHUNK)
    }
    pub fn expected(self, index: u8) -> Option<usize> {
        let start = index as usize * CHUNK;
        (start < self.len as usize).then(|| (self.len as usize - start).min(CHUNK))
    }
}

pub(crate) struct Tx {
    pub id: Identity,
    pub data: Vec<u8>,
    pub index: u8,
    pub deadline: std::time::Instant,
    pub keyed: bool,
    pub failed_exchanges: u8,
}

impl Tx {
    pub fn frame_payload(&self) -> Vec<u8> {
        let start = self.index as usize * CHUNK;
        let mut out = self.id.header(self.index);
        out.extend_from_slice(&self.data[start..start + self.id.expected(self.index).unwrap()]);
        out
    }
}

#[derive(Default)]
pub(crate) struct Rx {
    id: Option<Identity>,
    prefix: Vec<u8>,
}

pub(crate) enum Receive {
    Ignore,
    Progress(u8),
    Complete(TransportPdu),
}

impl Rx {
    pub fn reset(&mut self) {
        self.id = None;
        self.prefix.clear();
    }

    pub fn push(&mut self, id: Identity, index: u8, bytes: &[u8], expected_seq: u8) -> Receive {
        if id.seq != expected_seq || id.expected(index) != Some(bytes.len()) {
            return Receive::Ignore;
        }
        if self.id != Some(id) {
            if self.id.is_some() {
                return Receive::Ignore;
            }
            if index != 0 {
                return Receive::Progress(0);
            }
            self.reset();
            self.id = Some(id);
        }
        let next = self.prefix.len() / CHUNK;
        if index as usize > next {
            return Receive::Progress(0);
        }
        if (index as usize) < next {
            let start = index as usize * CHUNK;
            return if self.prefix[start..start + bytes.len()] == *bytes {
                Receive::Progress(index.saturating_add(1))
            } else {
                Receive::Ignore
            };
        }
        self.prefix.extend_from_slice(bytes);
        if self.prefix.len() < id.len as usize {
            return Receive::Progress(index + 1);
        }
        let result = if CRC.checksum(&self.prefix) == id.crc {
            TransportPdu::from_bytes(&self.prefix)
                .ok()
                .filter(|p| p.transport_type == TransportType::Reliable && p.seq_num == id.seq)
        } else {
            None
        };
        self.reset();
        result.map_or(Receive::Progress(0), Receive::Complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cop222_fragment_bounds_and_integrity() {
        assert!(Identity::new(&[0; 26], 0).is_none());
        assert!(Identity::new(&vec![0; 4104], 0).is_none());
        assert!(Identity::new(&[0; 27], 0).is_some());
        assert!(Identity::new(&[0; 4103], 0).is_some());
        let maximum = TransportPdu::new_reliable(0, 4, 0, vec![11; 4096]).to_bytes();
        let maximum_id = Identity::new(&maximum, 4).unwrap();
        assert_eq!(maximum_id.count(), 228);
        let mut max_rx = Rx::default();
        for (index, chunk) in maximum.chunks(CHUNK).enumerate() {
            let result = max_rx.push(maximum_id, index as u8, chunk, 4);
            if index == 227 {
                assert!(matches!(result, Receive::Complete(_)));
            }
        }
        let data = TransportPdu::new_reliable(0, 6, 0, vec![9; 252]).to_bytes();
        let id = Identity::new(&data, 6).unwrap();
        let mut rx = Rx::default();
        for (i, chunk) in data.chunks(CHUNK).enumerate() {
            let outcome = rx.push(id, i as u8, chunk, 6);
            if i + 1 == id.count() {
                assert!(matches!(outcome, Receive::Complete(_)));
            } else {
                assert!(matches!(outcome, Receive::Progress(_)));
            }
        }
        let mut rx = Rx::default();
        assert!(matches!(
            rx.push(id, 1, &data[CHUNK..2 * CHUNK], 6),
            Receive::Progress(0)
        ));
        assert!(matches!(
            rx.push(id, 0, &data[..CHUNK - 1], 6),
            Receive::Ignore
        ));
        assert!(matches!(rx.push(id, 0, &data[..CHUNK], 7), Receive::Ignore));
        assert!(matches!(
            rx.push(id, 0, &data[..CHUNK], 6),
            Receive::Progress(1)
        ));
        assert!(matches!(
            rx.push(id, 0, &data[..CHUNK], 6),
            Receive::Progress(1)
        ));
        let mut corrupt = data[CHUNK..2 * CHUNK].to_vec();
        corrupt[0] ^= 1;
        assert!(matches!(rx.push(id, 1, &corrupt, 6), Receive::Progress(2)));
        for (i, chunk) in data.chunks(CHUNK).enumerate().skip(2) {
            let result = rx.push(id, i as u8, chunk, 6);
            if i + 1 == id.count() {
                assert!(matches!(result, Receive::Progress(0)));
            }
        }
        let wrap = TransportPdu::new_reliable(1, 255, 0, vec![7; 20]).to_bytes();
        let wrap_id = Identity::new(&wrap, 255).unwrap();
        let mut rx = Rx::default();
        assert!(matches!(
            rx.push(wrap_id, 0, &wrap[..CHUNK], 255),
            Receive::Progress(1)
        ));
        match rx.push(wrap_id, 1, &wrap[CHUNK..], 255) {
            Receive::Complete(segment) => {
                assert_eq!(segment.seq_num, 255);
                assert_eq!(segment.session_id, 1);
            }
            _ => panic!("stream 1 across the wrapped sequence must reassemble"),
        }
    }
}
