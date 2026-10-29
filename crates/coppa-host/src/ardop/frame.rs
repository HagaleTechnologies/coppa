//! ARDOP data-port frame codec.
//!
//! The framing is verified against Pat's own client source
//! (`wl2k-go/transport/ardop/frame.go`), not the ticket text: host→TNC
//! frames are `<len:u16 big-endian><payload>` with `len` counting the
//! payload, and TNC→host frames add a 3-byte ASCII tag (`ARQ`/`FEC`/`ERR`/
//! `IDF`) after the length, with `len` counting the tag plus the payload.

use anyhow::{anyhow, Result};
use tokio::io::AsyncReadExt;

/// Largest frame the decoder will read: the whole `u16` length space. The
/// 2-byte length prefix bounds every allocation by construction, so a
/// hostile length can never make the reader allocate unbounded memory.
pub const MAX_FRAME: usize = u16::MAX as usize;

/// The TNC→host tag for ARQ session data.
pub const ARQ_TAG: &[u8; 3] = b"ARQ";

/// Encode one host→TNC frame: `<len:u16 BE><payload>`, `len` = payload
/// length.
pub fn encode_host_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(2 + payload.len());
    frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Encode one TNC→host frame: `<len:u16 BE><tag:3><payload>`, `len` =
/// 3 + payload length.
pub fn encode_tnc_frame(tag: &[u8; 3], payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(2 + 3 + payload.len());
    frame.extend_from_slice(&((3 + payload.len()) as u16).to_be_bytes());
    frame.extend_from_slice(tag);
    frame.extend_from_slice(payload);
    frame
}

/// Decode one TNC→host frame's parts from `bytes`: the tag and the payload
/// after byte 5 (Pat's `frame.go` semantics: `dataType: string(data[2:5])`,
/// `data: data[5:]`).
pub fn decode_tnc_frame(bytes: &[u8]) -> Result<(&str, &[u8])> {
    if bytes.len() < 2 {
        return Err(anyhow!("truncated frame: no length prefix"));
    }
    let len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    if len < 3 {
        return Err(anyhow!("frame length {len} carries no tag"));
    }
    if bytes.len() != 2 + len {
        return Err(anyhow!(
            "frame length {len} does not match the {} bytes present",
            bytes.len() - 2
        ));
    }
    let tag = std::str::from_utf8(&bytes[2..5]).map_err(|e| anyhow!("bad tag: {e}"))?;
    Ok((tag, &bytes[5..]))
}

/// Read exactly one host→TNC frame from `reader`: the 2-byte big-endian
/// length, then exactly that many payload bytes. A stream that ends mid-frame
/// is an error, never a half-delivered payload.
pub async fn read_host_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let mut prefix = [0u8; 2];
    reader.read_exact(&mut prefix).await?;
    let len = u16::from_be_bytes(prefix) as usize;
    if len > MAX_FRAME {
        return Err(anyhow!("frame length {len} exceeds MAX_FRAME"));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `<u16 BE><payload>` with `len == payload.len()`, splitting exactly on
    /// that boundary across chunks: two frames back to back plus a partial
    /// third decode as two frames and an error.
    #[tokio::test]
    async fn host_frame_round_trips_pat_length_semantics() {
        let two_and_a_half = [
            &encode_host_frame(b"demo")[..],
            &encode_host_frame(b"second")[..],
            // A partial third frame: the length prefix promises 4 bytes but
            // only 2 follow.
            &[0x00u8, 0x04, b't', b'h'][..],
        ]
        .concat();
        let mut stream = std::io::Cursor::new(two_and_a_half);
        // read_exact-style chunking: every byte is available at once, and
        // the decoder still splits on the length boundary.
        assert_eq!(
            read_host_frame(&mut stream).await.unwrap(),
            b"demo".to_vec()
        );
        assert_eq!(
            read_host_frame(&mut stream).await.unwrap(),
            b"second".to_vec()
        );
        // The partial third frame errors (truncated mid-payload), never
        // half-delivers.
        assert!(read_host_frame(&mut stream).await.is_err());
    }

    /// TNC→host frames carry the 3-byte ASCII tag after the length;
    /// `len == 3 + payload.len()` (Pat's `frame.go` reads exactly this).
    #[test]
    fn tnc_frame_carries_three_byte_tag() {
        let frame = encode_tnc_frame(ARQ_TAG, b"demo");
        assert_eq!(
            frame,
            [0x00, 0x07, b'A', b'R', b'Q', b'd', b'e', b'm', b'o']
        );
        let (tag, payload) = decode_tnc_frame(&frame).unwrap();
        assert_eq!(tag, "ARQ");
        assert_eq!(payload, b"demo");
        // An ERR-tagged frame decodes with tag ERR and the payload after
        // byte 5.
        let err = encode_tnc_frame(b"ERR", b"x");
        let (tag, payload) = decode_tnc_frame(&err).unwrap();
        assert_eq!(tag, "ERR");
        assert_eq!(payload, b"x");
    }

    /// The u16 length prefix bounds every allocation: a length beyond
    /// `MAX_FRAME` cannot be expressed, and a truncated frame errors.
    #[test]
    fn oversized_frame_is_rejected() {
        assert_eq!(MAX_FRAME, u16::MAX as usize);
        // len counts tag+payload, so the largest expressible TNC frame is
        // 65535 bytes total — bounded by construction.
        let frame = encode_tnc_frame(ARQ_TAG, &[0u8; MAX_FRAME - 3]);
        assert_eq!(frame.len(), 2 + MAX_FRAME);
        assert!(decode_tnc_frame(&frame).is_ok());
        // Truncated and inconsistent frames error, never half-deliver.
        assert!(decode_tnc_frame(&frame[..frame.len() - 1]).is_err());
        assert!(decode_tnc_frame(&frame[..1]).is_err());
        assert!(decode_tnc_frame(&[]).is_err());
    }
}
