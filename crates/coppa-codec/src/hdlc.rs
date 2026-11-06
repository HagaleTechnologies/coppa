//! Shared AX.25 HDLC framing.
use crc::{Crc, CRC_16_IBM_SDLC};
/// Convert a byte to 8 bits, LSB first (AX.25 standard).
pub(crate) fn byte_to_bits_lsb(byte: u8) -> [u8; 8] {
    let mut bits = [0u8; 8];
    for (i, b) in bits.iter_mut().enumerate() {
        *b = (byte >> i) & 1;
    }
    bits
}

/// Convert 8 bits (LSB first) back to a byte.
pub(crate) fn bits_to_byte_lsb(bits: &[u8]) -> u8 {
    debug_assert!(bits.len() >= 8);
    let mut byte = 0u8;
    for (i, &b) in bits.iter().enumerate().take(8) {
        byte |= b << i;
    }
    byte
}

/// Insert a zero bit after every run of five consecutive 1s (HDLC bit stuffing).
/// Operates on the frame body only; flag sequences are exempt.
pub(crate) fn bit_stuff(bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bits.len() + bits.len() / 5);
    let mut ones_run = 0u32;
    for &bit in bits {
        out.push(bit);
        if bit == 1 {
            ones_run += 1;
            if ones_run == 5 {
                out.push(0); // stuffed zero
                ones_run = 0;
            }
        } else {
            ones_run = 0;
        }
    }
    out
}

/// Remove stuffed zero bits.  Returns `None` if 6 or more consecutive 1s are
/// encountered (which would indicate a flag or an abort sequence, not data).
#[allow(dead_code)] // used in tests
pub(crate) fn bit_unstuff(bits: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(bits.len());
    let mut ones_run = 0u32;
    let mut i = 0;
    while i < bits.len() {
        let bit = bits[i];
        if bit == 1 {
            ones_run += 1;
            if ones_run == 6 {
                // Six consecutive 1s — invalid in data context.
                return None;
            }
            out.push(1);
        } else {
            if ones_run == 5 {
                // This is a stuffed zero — skip it (do not push).
                ones_run = 0;
                i += 1;
                continue;
            }
            ones_run = 0;
            out.push(0);
        }
        i += 1;
    }
    Some(out)
}

/// NRZI encode: bit 1 means *no change*, bit 0 means *transition*.
/// Initial state is `true` (mark frequency).
pub(crate) fn nrzi_encode(bits: &[u8]) -> Vec<bool> {
    let mut out = Vec::with_capacity(bits.len());
    let mut state = true;
    for &bit in bits {
        if bit == 0 {
            state = !state; // transition
        }
        // bit == 1: no change
        out.push(state);
    }
    out
}

/// NRZI decode: same consecutive value → 1, different → 0.
/// Previous symbol starts as `true` (mark).
#[allow(dead_code)] // used in tests
pub(crate) fn nrzi_decode(symbols: &[bool]) -> Vec<u8> {
    let mut out = Vec::with_capacity(symbols.len());
    let mut prev = true;
    for &sym in symbols {
        out.push(if sym == prev { 1 } else { 0 });
        prev = sym;
    }
    out
}

/// Append the AX.25 FCS, low byte first.
pub fn append_fcs(frame: &[u8]) -> Vec<u8> {
    let fcs = Crc::<u16>::new(&CRC_16_IBM_SDLC).checksum(frame);
    let mut out = Vec::with_capacity(frame.len() + 2);
    out.extend_from_slice(frame);
    out.extend_from_slice(&fcs.to_le_bytes());
    out
}

#[derive(Debug, Default)]
pub(crate) struct HdlcDecoder {
    in_frame: bool,
    bit_buffer: Vec<u8>,
    ones_count: u32,
    frames: Vec<Vec<u8>>,
}
impl HdlcDecoder {
    /// HDLC bit-level state machine using ones-counting approach.
    ///
    /// Strategy: track consecutive 1-bits. Only push data bits when we know
    /// they are not part of a flag or stuffing. The key rules:
    ///   - 1-bits with ones_count < 5: push as data, increment count
    ///   - 0-bit after ones_count == 5: stuffed zero, discard (data 1s already pushed)
    ///   - 0-bit after ones_count == 6: flag! Remove last 5 pushed 1s from buffer
    ///   - ones_count reaches 7+: abort
    pub(crate) fn process_bit(&mut self, bit: u8) {
        if bit == 1 {
            self.ones_count += 1;
            if self.ones_count >= 7 {
                // Abort: 7+ consecutive 1s
                self.in_frame = false;
                self.bit_buffer.clear();
                return;
            }
            // Push the 1-bit as data. For the 6th consecutive 1, we also push
            // it tentatively; if it turns out to be part of a flag (next bit 0),
            // we'll remove 6 ones from the buffer.
            if self.in_frame {
                self.bit_buffer.push(1);
            }
        } else {
            // bit == 0
            if self.ones_count >= 7 {
                // Recovery from abort
                self.ones_count = 0;
                return;
            }

            if self.ones_count == 6 {
                // Flag detected (01111110).
                // We pushed 7 bits into bit_buffer as data: the leading 0 of the
                // flag plus the 6 ones. Remove all 7.
                if self.in_frame {
                    let buf_len = self.bit_buffer.len();
                    let remove = 7.min(buf_len);
                    self.bit_buffer.truncate(buf_len - remove);
                    if !self.bit_buffer.is_empty() {
                        self.try_extract_frame();
                    }
                }
                self.in_frame = true;
                self.bit_buffer.clear();
                self.ones_count = 0;
                return;
            }

            if self.ones_count == 5 {
                // Stuffed zero — the 5 ones were real data (already pushed).
                // Discard this 0.
                self.ones_count = 0;
                return;
            }

            // Normal 0 bit
            self.ones_count = 0;
            if self.in_frame {
                self.bit_buffer.push(0);
            }
        }
    }

    /// Attempt to extract a frame from the accumulated bit buffer.
    fn try_extract_frame(&mut self) {
        // Need at least 16 bits for 2-byte CRC
        if self.bit_buffer.len() < 16 {
            return;
        }

        // Must be a multiple of 8 bits
        let bit_len = self.bit_buffer.len();
        if bit_len % 8 != 0 {
            return;
        }

        let num_bytes = bit_len / 8;
        let mut bytes = Vec::with_capacity(num_bytes);
        for i in 0..num_bytes {
            bytes.push(bits_to_byte_lsb(&self.bit_buffer[i * 8..(i + 1) * 8]));
        }

        // CRC verification: last 2 bytes are FCS (low byte first)
        if bytes.len() < 3 {
            return; // Need at least 1 data byte + 2 CRC bytes
        }

        let data = &bytes[..bytes.len() - 2];
        let fcs_lo = bytes[bytes.len() - 2];
        let fcs_hi = bytes[bytes.len() - 1];
        let received_fcs = u16::from_le_bytes([fcs_lo, fcs_hi]);

        let crc_alg = Crc::<u16>::new(&CRC_16_IBM_SDLC);
        let computed_fcs = crc_alg.checksum(data);

        if computed_fcs == received_fcs {
            // Push the full frame including CRC (as the modulator expects)
            self.frames.push(bytes);
        }
    }

    /// Extract all completed frames, draining the internal buffer.
    pub(crate) fn take_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.frames)
    }
}
