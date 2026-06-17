//! AX.25 UI frame codec — byte-level encoding and decoding.
//!
//! This module handles the AX.25 frame structure including callsign addressing
//! and CRC-16. HDLC bit-level framing (flags, bit stuffing, NRZI) is handled
//! separately by the AFSK modem.
//!
//! Frame layout:
//! ```text
//! | Dest Addr (7) | Src Addr (7) | Digipeaters (0–56) | Ctrl 0x03 | PID 0xF0 | Info (0–256) | FCS (2) |
//! ```

use crc::{Crc, CRC_16_IBM_SDLC};

/// Maximum info field length (bytes).
pub const MAX_INFO_LEN: usize = 256;

/// Maximum number of digipeater addresses.
pub const MAX_DIGIPEATERS: usize = 8;

const AX25_CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);

// ── Address ──────────────────────────────────────────────────────────────────

/// An AX.25 address (callsign + SSID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ax25Address {
    /// Callsign, e.g. `"W1AW"`.  At most 6 ASCII uppercase characters.
    pub callsign: String,
    /// Secondary Station Identifier (0–15).
    pub ssid: u8,
}

impl Ax25Address {
    /// Encode the address into 7 bytes.
    ///
    /// Each callsign character is ASCII left-shifted by 1 bit.  The callsign
    /// is space-padded (or truncated) to exactly 6 characters.  The 7th byte
    /// is the SSID byte: `0x60 | ((ssid & 0x0F) << 1) | last_flag`.
    pub fn to_bytes(&self, last: bool) -> [u8; 7] {
        let mut out = [0u8; 7];
        // Build a 6-byte, space-padded callsign field.
        let padded: Vec<u8> = {
            let mut v: Vec<u8> = self.callsign.bytes().take(6).collect();
            while v.len() < 6 {
                v.push(b' ');
            }
            v
        };
        for (i, &ch) in padded.iter().enumerate() {
            out[i] = ch << 1;
        }
        let last_bit: u8 = if last { 1 } else { 0 };
        out[6] = 0x60 | ((self.ssid & 0x0F) << 1) | last_bit;
        out
    }

    /// Decode an address from a 7-byte slice.  Returns `None` if the slice is
    /// shorter than 7 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 7 {
            return None;
        }
        // Each character byte is right-shifted by 1 to recover the ASCII value.
        let callsign: String = bytes[..6]
            .iter()
            .map(|&b| (b >> 1) as char)
            .collect::<String>()
            .trim_end()
            .to_owned();
        let ssid = (bytes[6] >> 1) & 0x0F;
        Some(Self { callsign, ssid })
    }

    /// Return `true` when the last-address bit is set in the SSID byte.
    pub(crate) fn is_last(bytes: &[u8]) -> bool {
        bytes.len() >= 7 && (bytes[6] & 0x01) != 0
    }
}

// ── Frame ─────────────────────────────────────────────────────────────────────

/// An AX.25 UI frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ax25Frame {
    pub dest: Ax25Address,
    pub src: Ax25Address,
    /// Up to [`MAX_DIGIPEATERS`] digipeater addresses.
    pub digipeaters: Vec<Ax25Address>,
    /// Information field (0–[`MAX_INFO_LEN`] bytes).
    pub info: Vec<u8>,
}

impl Ax25Frame {
    /// Encode the frame to bytes (without HDLC flags or bit stuffing).
    ///
    /// Layout: addresses | 0x03 (UI control) | 0xF0 (no-layer-3 PID) | info | FCS (low, high)
    pub fn to_bytes(&self) -> Vec<u8> {
        let digi_count = self.digipeaters.len().min(MAX_DIGIPEATERS);
        let mut buf: Vec<u8> = Vec::with_capacity(14 + digi_count * 7 + 2 + self.info.len() + 2);

        // Determine which address is the last: destination is never last unless
        // there are no subsequent addresses (impossible in valid AX.25), source
        // is last when there are no digipeaters, otherwise the final digipeater
        // is last.
        let has_digis = digi_count > 0;

        buf.extend_from_slice(&self.dest.to_bytes(false));
        buf.extend_from_slice(&self.src.to_bytes(!has_digis));

        for (i, digi) in self.digipeaters.iter().take(digi_count).enumerate() {
            let is_last_digi = i == digi_count - 1;
            buf.extend_from_slice(&digi.to_bytes(is_last_digi));
        }

        buf.push(0x03); // Control: UI frame
        buf.push(0xF0); // PID: no layer 3

        let info_len = self.info.len().min(MAX_INFO_LEN);
        buf.extend_from_slice(&self.info[..info_len]);

        // FCS: CRC-16-IBM-SDLC over everything before FCS, low byte first.
        let crc = AX25_CRC.checksum(&buf);
        buf.push((crc & 0xFF) as u8);
        buf.push((crc >> 8) as u8);

        buf
    }

    /// Decode a frame from bytes, verifying the FCS.  Returns `None` on any
    /// parse or CRC error.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        // Minimum: dest(7) + src(7) + ctrl(1) + pid(1) + fcs(2) = 18 bytes.
        if bytes.len() < 18 {
            return None;
        }

        // Verify FCS first.
        let fcs_pos = bytes.len() - 2;
        let stored_crc = (bytes[fcs_pos] as u16) | ((bytes[fcs_pos + 1] as u16) << 8);
        let computed_crc = AX25_CRC.checksum(&bytes[..fcs_pos]);
        if stored_crc != computed_crc {
            return None;
        }

        // Parse addresses.  Walk 7-byte chunks until the last-address bit is set.
        let mut offset = 0usize;

        let dest = Ax25Address::from_bytes(&bytes[offset..offset + 7])?;
        // Destination's last bit should not be set, but we don't enforce it
        // strictly — some implementations set it incorrectly.
        offset += 7;

        let src = Ax25Address::from_bytes(&bytes[offset..offset + 7])?;
        let src_is_last = Ax25Address::is_last(&bytes[offset..offset + 7]);
        offset += 7;

        let mut digipeaters = Vec::new();
        if !src_is_last {
            loop {
                if offset + 7 > fcs_pos {
                    // Ran out of address space before finding last-address bit.
                    return None;
                }
                let chunk = &bytes[offset..offset + 7];
                let is_last = Ax25Address::is_last(chunk);
                let digi = Ax25Address::from_bytes(chunk)?;
                digipeaters.push(digi);
                offset += 7;
                if is_last {
                    break;
                }
                if digipeaters.len() >= MAX_DIGIPEATERS {
                    return None;
                }
            }
        }

        // Control and PID bytes.
        if offset + 2 > fcs_pos {
            return None;
        }
        let ctrl = bytes[offset];
        let pid = bytes[offset + 1];
        if ctrl != 0x03 || pid != 0xF0 {
            return None; // Only UI frames with no-L3 protocol supported
        }
        offset += 2;

        let info = bytes[offset..fcs_pos].to_vec();

        Some(Self {
            dest,
            src,
            digipeaters,
            info,
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Address round-trips ───────────────────────────────────────────────────

    #[test]
    fn address_roundtrip_normal() {
        let addr = Ax25Address {
            callsign: "W1AW".to_owned(),
            ssid: 0,
        };
        let bytes = addr.to_bytes(false);
        let decoded = Ax25Address::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, addr);
        assert!(!Ax25Address::is_last(&bytes));
    }

    #[test]
    fn address_roundtrip_short_callsign() {
        let addr = Ax25Address {
            callsign: "KD9".to_owned(),
            ssid: 3,
        };
        let bytes = addr.to_bytes(false);
        let decoded = Ax25Address::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, addr);
    }

    #[test]
    fn address_roundtrip_with_ssid() {
        let addr = Ax25Address {
            callsign: "N0CALL".to_owned(),
            ssid: 15,
        };
        let bytes = addr.to_bytes(true);
        let decoded = Ax25Address::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, addr);
        assert!(Ax25Address::is_last(&bytes));
    }

    #[test]
    fn address_last_bit_false() {
        let addr = Ax25Address {
            callsign: "KA1ABC".to_owned(),
            ssid: 7,
        };
        let bytes = addr.to_bytes(false);
        assert!(!Ax25Address::is_last(&bytes));
        let decoded = Ax25Address::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.ssid, 7);
    }

    // ── Frame round-trips ─────────────────────────────────────────────────────

    fn make_frame_no_digis() -> Ax25Frame {
        Ax25Frame {
            dest: Ax25Address {
                callsign: "APRS".to_owned(),
                ssid: 0,
            },
            src: Ax25Address {
                callsign: "W1AW".to_owned(),
                ssid: 0,
            },
            digipeaters: vec![],
            info: b"Hello, AX.25!".to_vec(),
        }
    }

    #[test]
    fn frame_roundtrip_no_digipeaters() {
        let frame = make_frame_no_digis();
        let bytes = frame.to_bytes();
        let decoded = Ax25Frame::from_bytes(&bytes).expect("decode failed");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn frame_roundtrip_with_digipeaters() {
        let frame = Ax25Frame {
            dest: Ax25Address {
                callsign: "APRS".to_owned(),
                ssid: 0,
            },
            src: Ax25Address {
                callsign: "W1AW".to_owned(),
                ssid: 1,
            },
            digipeaters: vec![
                Ax25Address {
                    callsign: "RELAY1".to_owned(),
                    ssid: 0,
                },
                Ax25Address {
                    callsign: "WIDE2".to_owned(),
                    ssid: 2,
                },
            ],
            info: b"Test with digipeaters".to_vec(),
        };
        let bytes = frame.to_bytes();
        let decoded = Ax25Frame::from_bytes(&bytes).expect("decode failed");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn frame_roundtrip_empty_info() {
        let frame = Ax25Frame {
            dest: Ax25Address {
                callsign: "NOCALL".to_owned(),
                ssid: 0,
            },
            src: Ax25Address {
                callsign: "KD9XYZ".to_owned(),
                ssid: 0,
            },
            digipeaters: vec![],
            info: vec![],
        };
        let bytes = frame.to_bytes();
        let decoded = Ax25Frame::from_bytes(&bytes).expect("decode failed");
        assert_eq!(decoded, frame);
    }

    // ── CRC corruption detection ──────────────────────────────────────────────

    #[test]
    fn crc_corruption_detected() {
        let frame = make_frame_no_digis();
        let mut bytes = frame.to_bytes();
        // Flip a byte in the info field.
        let flip_pos = bytes.len() / 2;
        bytes[flip_pos] ^= 0xFF;
        assert!(
            Ax25Frame::from_bytes(&bytes).is_none(),
            "corrupted frame should fail CRC check"
        );
    }

    #[test]
    fn crc_fcs_corruption_detected() {
        let frame = make_frame_no_digis();
        let mut bytes = frame.to_bytes();
        // Corrupt the FCS bytes directly.
        let len = bytes.len();
        bytes[len - 1] ^= 0x01;
        assert!(
            Ax25Frame::from_bytes(&bytes).is_none(),
            "corrupted FCS should fail CRC check"
        );
    }

    #[test]
    fn too_short_bytes_returns_none() {
        assert!(Ax25Frame::from_bytes(&[0u8; 10]).is_none());
    }

    // ── Digipeater limit enforcement ──────────────────────────────────────────

    /// Build raw AX.25 bytes for a frame with exactly `n` digipeaters,
    /// bypassing `Ax25Frame::to_bytes` so the truncation cap is not applied.
    /// The FCS is correctly computed so the *only* reason `from_bytes` would
    /// reject the result is the digipeater count limit.
    fn raw_frame_with_n_digis(n: usize) -> Vec<u8> {
        let dest = Ax25Address {
            callsign: "APRS".to_owned(),
            ssid: 0,
        };
        let src = Ax25Address {
            callsign: "W1AW".to_owned(),
            ssid: 0,
        };
        let digipeaters: Vec<Ax25Address> = (0..n)
            .map(|i| Ax25Address {
                callsign: format!("DIG{:03}", i),
                ssid: 0,
            })
            .collect();

        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&dest.to_bytes(false));
        buf.extend_from_slice(&src.to_bytes(n == 0));
        for (i, digi) in digipeaters.iter().enumerate() {
            buf.extend_from_slice(&digi.to_bytes(i == n - 1));
        }
        buf.push(0x03); // Control: UI
        buf.push(0xF0); // PID: no layer 3
        buf.extend_from_slice(b"payload");
        let crc = AX25_CRC.checksum(&buf);
        buf.push((crc & 0xFF) as u8);
        buf.push((crc >> 8) as u8);
        buf
    }

    #[test]
    fn frame_roundtrip_max_digipeaters() {
        // A frame with exactly MAX_DIGIPEATERS (8) digipeaters must round-trip.
        let frame = Ax25Frame {
            dest: Ax25Address {
                callsign: "APRS".to_owned(),
                ssid: 0,
            },
            src: Ax25Address {
                callsign: "W1AW".to_owned(),
                ssid: 0,
            },
            digipeaters: (0..MAX_DIGIPEATERS)
                .map(|i| Ax25Address {
                    callsign: format!("DIG{:03}", i),
                    ssid: 0,
                })
                .collect(),
            info: b"8 digis".to_vec(),
        };
        let bytes = frame.to_bytes();
        let decoded = Ax25Frame::from_bytes(&bytes).expect("8-digipeater frame should decode");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn from_bytes_rejects_nine_digipeaters() {
        // Raw bytes with 9 digipeaters and a correct FCS — from_bytes must
        // reject because the count exceeds MAX_DIGIPEATERS (8).
        let raw = raw_frame_with_n_digis(9);
        assert!(
            Ax25Frame::from_bytes(&raw).is_none(),
            "frame with 9 digipeaters should be rejected by from_bytes"
        );
    }

    // ── Control / PID validation ──────────────────────────────────────────────

    /// Build a raw frame with an arbitrary control byte (correct FCS included).
    fn raw_frame_with_ctrl_pid(ctrl: u8, pid: u8) -> Vec<u8> {
        let dest = Ax25Address {
            callsign: "APRS".to_owned(),
            ssid: 0,
        };
        let src = Ax25Address {
            callsign: "W1AW".to_owned(),
            ssid: 0,
        };
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&dest.to_bytes(false));
        buf.extend_from_slice(&src.to_bytes(true)); // src is last address
        buf.push(ctrl);
        buf.push(pid);
        buf.extend_from_slice(b"payload");
        let crc = AX25_CRC.checksum(&buf);
        buf.push((crc & 0xFF) as u8);
        buf.push((crc >> 8) as u8);
        buf
    }

    #[test]
    fn from_bytes_rejects_non_ui_control_byte() {
        // 0x00 is a valid SABM-like control byte but not a UI frame (0x03).
        let raw = raw_frame_with_ctrl_pid(0x00, 0xF0);
        assert!(
            Ax25Frame::from_bytes(&raw).is_none(),
            "non-UI control byte should cause from_bytes to return None"
        );
    }

    #[test]
    fn from_bytes_rejects_non_no_l3_pid() {
        // Control is correct UI (0x03) but PID is not no-L3 (0xF0).
        let raw = raw_frame_with_ctrl_pid(0x03, 0xCF);
        assert!(
            Ax25Frame::from_bytes(&raw).is_none(),
            "non-no-L3 PID should cause from_bytes to return None"
        );
    }

    #[test]
    fn from_bytes_accepts_valid_ui_frame() {
        // Sanity check: correct ctrl+pid still decodes successfully.
        let raw = raw_frame_with_ctrl_pid(0x03, 0xF0);
        assert!(
            Ax25Frame::from_bytes(&raw).is_some(),
            "valid UI frame must decode successfully"
        );
    }
}
