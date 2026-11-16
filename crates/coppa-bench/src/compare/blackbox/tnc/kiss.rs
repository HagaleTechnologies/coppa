//! KISS framing, from the public KISS TNC protocol paper.

pub const FEND: u8 = 0xc0;
pub const FESC: u8 = 0xdb;
pub const TFEND: u8 = 0xdc;
pub const TFESC: u8 = 0xdd;

pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![FEND, 0];
    for &b in payload {
        match b {
            FEND => out.extend_from_slice(&[FESC, TFEND]),
            FESC => out.extend_from_slice(&[FESC, TFESC]),
            _ => out.push(b),
        }
    }
    out.push(FEND);
    out
}

pub fn decode(frame: &[u8]) -> Option<Vec<u8>> {
    let bytes = frame.strip_prefix(&[FEND, 0])?.strip_suffix(&[FEND])?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == FESC {
            i += 1;
            out.push(match *bytes.get(i)? {
                TFEND => FEND,
                TFESC => FESC,
                _ => return None,
            });
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    Some(out)
}

pub fn frames(payload: &[u8], datagram_bytes: usize) -> Vec<Vec<u8>> {
    if datagram_bytes == 0 {
        return Vec::new();
    }
    payload.chunks(datagram_bytes).map(frame).collect()
}

pub fn drain_frames(wire: &mut Vec<u8>, payload: &mut Vec<u8>) -> usize {
    let mut count = 0;
    loop {
        let Some(start) = wire.iter().position(|&b| b == FEND) else {
            wire.clear();
            break;
        };
        if start > 0 {
            wire.drain(..start);
        }
        let Some(end) = wire[1..].iter().position(|&b| b == FEND).map(|p| p + 1) else {
            break;
        };
        if let Some(bytes) = decode(&wire[..=end]) {
            payload.extend_from_slice(&bytes);
            count += 1;
        }
        wire.drain(..=end);
    }
    count
}

pub fn lost_datagrams(size_bytes: usize, datagram_bytes: usize, received: usize) -> usize {
    if datagram_bytes == 0 {
        return 0;
    }
    size_bytes.div_ceil(datagram_bytes).saturating_sub(received)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fend_fesc_escaping_round_trips() {
        let p = [1, FEND, 2, FESC];
        assert_eq!(decode(&frame(&p)).unwrap(), p);
    }
    #[test]
    fn payload_splits_at_datagram_bytes() {
        assert_eq!(frames(&[0; 2000], 900).len(), 3);
    }
    #[test]
    fn stream_decodes_multiple_datagrams() {
        let mut wire = frame(b"ab");
        wire.extend(frame(b"cd"));
        let mut payload = Vec::new();
        assert_eq!(drain_frames(&mut wire, &mut payload), 2);
        assert_eq!(payload, b"abcd");
    }
    #[test]
    fn lost_datagrams_count_as_drops() {
        assert_eq!(lost_datagrams(2048, 900, 2), 1);
        assert_eq!(lost_datagrams(2048, 900, 3), 0);
    }
}
