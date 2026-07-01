//! FEC + CRC + interleaving wrapper for the 48-bit frame header.
//!
//! The bare header is hard-decision BPSK with no protection; under HF fading that
//! unprotected header is ~89% of frame loss (BENCHMARKS "The frame header is the
//! dominant fading failure"). This module protects it:
//!   48 header bits + CRC-16 (16) + zero pad (8) = 72 info bits
//!   -> 6 x Golay(24,12) = 144 coded bits
//!   -> stride-5 interleave (spreads fading nulls across Golay words)
//!   -> BPSK.
//! On decode, any uncorrectable Golay word, nonzero pad, or CRC mismatch drops the
//! frame (returns None) rather than mis-decoding it.

use crate::ofdm::frame::CoppaHeader;
use crate::ofdm::golay::{golay24_decode, golay24_encode};
use crc::{Crc, CRC_16_IBM_SDLC};

const CRC16: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);

/// Number of coded BPSK bits the protected header occupies (6 Golay words * 24).
pub const PROTECTED_HEADER_CODED_BITS: usize = 144;

const INFO_BITS: usize = 72; // 48 header + 16 CRC + 8 pad
const N_WORDS: usize = 6;
/// Interleave stride. gcd(5,144)=1 (a bijection); chosen by a measured null-spreading
/// check so both a persistent single-carrier null and a wide within-symbol null keep
/// <= 3 errors in any single Golay word (its correction budget).
const INTERLEAVE_STRIDE: usize = 5;

fn interleave(coded: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; PROTECTED_HEADER_CODED_BITS];
    for (i, &bit) in coded.iter().enumerate().take(PROTECTED_HEADER_CODED_BITS) {
        out[(i * INTERLEAVE_STRIDE) % PROTECTED_HEADER_CODED_BITS] = bit;
    }
    out
}

fn deinterleave(rx: &[u8]) -> Vec<u8> {
    let mut coded = vec![0u8; PROTECTED_HEADER_CODED_BITS];
    for (i, slot) in coded.iter_mut().enumerate() {
        *slot = rx[(i * INTERLEAVE_STRIDE) % PROTECTED_HEADER_CODED_BITS];
    }
    coded
}

/// Encode a header into 144 interleaved coded bits (each 0 or 1), ready for BPSK.
pub fn encode_header(header: &CoppaHeader) -> Vec<u8> {
    let hb = header.to_bytes();
    let crc = CRC16.checksum(&hb);

    // 72 info bits, MSB-first: 48 header + 16 CRC + 8 zero pad.
    let mut info = Vec::with_capacity(INFO_BITS);
    for &byte in &hb {
        for shift in (0..8).rev() {
            info.push((byte >> shift) & 1);
        }
    }
    for shift in (0..16).rev() {
        info.push(((crc >> shift) & 1) as u8);
    }
    info.resize(INFO_BITS, 0);

    // 6 words of 12 bits -> Golay(24) -> 144 coded bits.
    let mut coded = Vec::with_capacity(PROTECTED_HEADER_CODED_BITS);
    for w in 0..N_WORDS {
        let mut word: u16 = 0;
        for k in 0..12 {
            word = (word << 1) | info[w * 12 + k] as u16;
        }
        let cw = golay24_encode(word);
        for b in (0..24).rev() {
            coded.push(((cw >> b) & 1) as u8);
        }
    }

    interleave(&coded)
}

/// Decode 144 received coded bits (0/1, in transmitted order) into a header.
/// Returns `None` if any Golay word is uncorrectable, the pad is nonzero, or the CRC
/// fails — in every failure case the frame is dropped, never mis-decoded.
pub fn decode_header(coded_bits: &[u8]) -> Option<CoppaHeader> {
    if coded_bits.len() < PROTECTED_HEADER_CODED_BITS {
        return None;
    }
    let deint = deinterleave(&coded_bits[..PROTECTED_HEADER_CODED_BITS]);

    let mut info = Vec::with_capacity(INFO_BITS);
    for w in 0..N_WORDS {
        let mut cw: u32 = 0;
        for b in 0..24 {
            cw = (cw << 1) | deint[w * 24 + b] as u32;
        }
        let (word, _n) = golay24_decode(cw)?;
        for k in (0..12).rev() {
            info.push(((word >> k) & 1) as u8);
        }
    }

    // Pad bits (info[64..72]) must be zero.
    if info[64..72].iter().any(|&b| b != 0) {
        return None;
    }

    // Repack 48 header bits -> 6 bytes; verify CRC-16 over them.
    let mut hb = [0u8; 6];
    for (i, byte) in hb.iter_mut().enumerate() {
        for k in 0..8 {
            *byte |= (info[i * 8 + k] & 1) << (7 - k);
        }
    }
    let mut crc_rx: u16 = 0;
    for k in 0..16 {
        crc_rx = (crc_rx << 1) | info[48 + k] as u16;
    }
    if CRC16.checksum(&hb) != crc_rx {
        return None;
    }

    CoppaHeader::from_bytes(&hb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ofdm::frame::CoppaFrameType;

    fn sample() -> CoppaHeader {
        CoppaHeader {
            version: 2,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 3,
            fec_type: 0,
            speed_level: 6,
            seq_num: 42,
            payload_len: 1500,
        }
    }

    #[test]
    fn clean_roundtrip() {
        let h = sample();
        let coded = encode_header(&h);
        assert_eq!(coded.len(), PROTECTED_HEADER_CODED_BITS);
        assert!(coded.iter().all(|&b| b <= 1));
        assert_eq!(decode_header(&coded), Some(h));
    }

    #[test]
    fn interleave_is_a_bijection() {
        let src: Vec<u8> = (0..PROTECTED_HEADER_CODED_BITS as u16)
            .map(|i| (i % 2) as u8)
            .collect();
        assert_eq!(deinterleave(&interleave(&src)), src);
    }

    #[test]
    fn corrects_up_to_three_errors_in_one_word_after_deinterleave() {
        let h = sample();
        let coded = encode_header(&h);
        let mut deint = deinterleave(&coded);
        deint[0] ^= 1;
        deint[5] ^= 1;
        deint[23] ^= 1;
        let reint = interleave(&deint);
        assert_eq!(decode_header(&reint), Some(h));
    }

    #[test]
    fn crc_rejects_uncorrectable_corruption() {
        let h = sample();
        let coded = encode_header(&h);
        let mut deint = deinterleave(&coded);
        for i in [0usize, 1, 2, 3] {
            deint[i] ^= 1;
        }
        let reint = interleave(&deint);
        assert_eq!(decode_header(&reint), None);
    }

    #[test]
    fn too_few_bits_is_none() {
        assert_eq!(decode_header(&[0u8; 100]), None);
    }
}
