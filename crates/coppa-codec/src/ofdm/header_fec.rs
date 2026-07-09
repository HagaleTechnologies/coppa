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
//!
//! Two decoders share this framing: [`decode_header`], the original hard-decision
//! Golay decoder (kept as a reference implementation and as the "hard" side of
//! soft-vs-hard comparisons), and [`decode_header_soft`], the soft-ML + CRC-assisted
//! list decoder actually used on the live receive path (`CoppaModem::demodulate_header`
//! feeds it LLRs instead of hard-sliced bits -- see that method's doc).

use crate::ofdm::frame::CoppaHeader;
use crate::ofdm::golay::{golay24_decode, golay24_decode_soft, golay24_encode};
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

/// `f32` (LLR-domain) mirror of [`deinterleave`]: same stride permutation, used by
/// [`decode_header_soft`] to undo the TX-side bit interleave on received LLRs
/// (rather than hard 0/1 bits) before per-word soft-ML Golay decoding.
fn deinterleave_llrs(rx: &[f32]) -> Vec<f32> {
    let mut coded = vec![0.0f32; PROTECTED_HEADER_CODED_BITS];
    for (i, slot) in coded.iter_mut().enumerate() {
        *slot = rx[(i * INTERLEAVE_STRIDE) % PROTECTED_HEADER_CODED_BITS];
    }
    coded
}

/// Test-only inverse of [`deinterleave_llrs`] (`f32` mirror of [`interleave`]) --
/// production code never needs to re-interleave LLRs (they arrive in transmitted/
/// interleaved order already), but tests that want to corrupt specific LLRs in the
/// *deinterleaved*, per-Golay-word domain (e.g. "zero 7 LLRs of word 0") need to map
/// back to transmitted order before calling `decode_header_soft`.
#[cfg(test)]
fn interleave_llrs(coded: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; PROTECTED_HEADER_CODED_BITS];
    for (i, &v) in coded.iter().enumerate().take(PROTECTED_HEADER_CODED_BITS) {
        out[(i * INTERLEAVE_STRIDE) % PROTECTED_HEADER_CODED_BITS] = v;
    }
    out
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

/// Validate pad bits + CRC-16 for 72 decoded info bits and parse the resulting 6
/// header bytes. Shared finishing step for both [`decode_header`] (hard) and
/// [`decode_header_soft`] (soft-ML + CRC-assisted list): each only differs in how it
/// gets from received bits/LLRs to a candidate `info` vector; both hand that off
/// here to check the pad and CRC and build the `CoppaHeader`. Returns `None` on a
/// nonzero pad, a CRC mismatch, or an invalid frame type -- every failure drops the
/// frame rather than mis-decoding it.
fn finish_decode(info: &[u8]) -> Option<CoppaHeader> {
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

    finish_decode(&info)
}

/// Soft header decode: deinterleave 144 received LLRs, soft-ML decode each of the 6
/// Golay(24,12) words to a list of its 2 best-scoring info words
/// ([`golay24_decode_soft`]), then select the combination that passes CRC-16 --
/// greedy: try all best-scoring words first, then flip each word to its runner-up
/// one at a time (6 trials), then flip pairs of words together (15 trials). At most
/// `1 + 6 + 15 = 22` CRC checks. Returns the first combination whose pad is zero and
/// CRC-16 matches; if none of the 22 combinations pass, returns `None` (frame
/// dropped, never mis-decoded) -- CRC-16 makes a false accept astronomically
/// unlikely, so trying combinations in a fixed, unweighted order (rather than
/// ranking by combined score first) doesn't risk correctness, only wasted CRC
/// checks in the rare case more than one word's best guess is wrong.
pub fn decode_header_soft(llrs: &[f32]) -> Option<CoppaHeader> {
    if llrs.len() < PROTECTED_HEADER_CODED_BITS {
        return None;
    }
    let deint = deinterleave_llrs(&llrs[..PROTECTED_HEADER_CODED_BITS]);

    let mut candidates: Vec<[(u16, f32); 2]> = Vec::with_capacity(N_WORDS);
    for w in 0..N_WORDS {
        let mut word_llrs = [0.0f32; 24];
        word_llrs.copy_from_slice(&deint[w * 24..w * 24 + 24]);
        candidates.push(golay24_decode_soft(&word_llrs));
    }

    let info_for = |picks: [usize; N_WORDS]| -> Vec<u8> {
        let mut info = Vec::with_capacity(INFO_BITS);
        for (w, &pick) in picks.iter().enumerate() {
            let word = candidates[w][pick].0;
            for k in (0..12).rev() {
                info.push(((word >> k) & 1) as u8);
            }
        }
        info
    };

    // 1. All best-scoring words.
    if let Some(h) = finish_decode(&info_for([0; N_WORDS])) {
        return Some(h);
    }
    // 2. Flip each word to its runner-up, one at a time (6 trials).
    for i in 0..N_WORDS {
        let mut picks = [0usize; N_WORDS];
        picks[i] = 1;
        if let Some(h) = finish_decode(&info_for(picks)) {
            return Some(h);
        }
    }
    // 3. Flip pairs of words together (15 trials).
    for i in 0..N_WORDS {
        for j in (i + 1)..N_WORDS {
            let mut picks = [0usize; N_WORDS];
            picks[i] = 1;
            picks[j] = 1;
            if let Some(h) = finish_decode(&info_for(picks)) {
                return Some(h);
            }
        }
    }
    None
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
            codewords: 1,
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

    /// Turns transmitted (interleaved) 0/1 bits into strong bipolar LLRs, +4 for a 0
    /// bit and -4 for a 1 bit -- a clean, high-confidence channel.
    fn strong_llrs(coded: &[u8]) -> Vec<f32> {
        coded
            .iter()
            .map(|&b| if b == 0 { 4.0 } else { -4.0 })
            .collect()
    }

    #[test]
    fn soft_ml_matches_hard_on_clean_words() {
        let headers = [
            sample(),
            CoppaHeader {
                version: 0,
                phy_mode: 0,
                frame_type: CoppaFrameType::Ack,
                bandwidth: 0,
                fec_type: 0,
                speed_level: 0,
                seq_num: 0,
                payload_len: 0,
                codewords: 1,
            },
            CoppaHeader {
                version: 15,
                phy_mode: 15,
                frame_type: CoppaFrameType::Beacon,
                bandwidth: 15,
                fec_type: 15,
                speed_level: 15,
                seq_num: 255,
                payload_len: 4095,
                codewords: 1,
            },
            CoppaHeader {
                version: 5,
                phy_mode: 2,
                frame_type: CoppaFrameType::Connect,
                bandwidth: 7,
                fec_type: 3,
                speed_level: 9,
                seq_num: 128,
                payload_len: 777,
                codewords: 1,
            },
        ];
        for h in headers {
            let coded = encode_header(&h);
            let llrs = strong_llrs(&coded);
            assert_eq!(
                decode_header_soft(&llrs),
                Some(h.clone()),
                "soft decode should match the hard decode on a clean, high-confidence \
                 channel for {h:?}"
            );
            assert_eq!(decode_header(&coded), Some(h));
        }
    }

    #[test]
    fn soft_ml_survives_seven_erasures_per_word() {
        // The hard decoder's correction budget is 3 errors per Golay(24,12) word;
        // soft-ML decoding, given only erasures (LLR = 0, no wrong-sign information),
        // survives up to d_min - 1 = 7 on an otherwise-clean word (see
        // `golay24_decode_soft`'s doc). Erase 7 of word 0's 24 LLRs (in the
        // deinterleaved, per-word domain) and confirm `decode_header_soft` still
        // recovers the header.
        let h = sample();
        let coded = encode_header(&h);
        let llrs = strong_llrs(&coded);

        let mut deint = deinterleave_llrs(&llrs);
        for slot in deint.iter_mut().take(7) {
            // word 0 occupies deint[0..24]
            *slot = 0.0;
        }
        let reint = interleave_llrs(&deint);

        assert_eq!(
            decode_header_soft(&reint),
            Some(h),
            "soft decode should survive 7 erasures in a single Golay word"
        );
    }

    #[test]
    fn crc_list_rescues_a_wrong_best_word() {
        // Craft LLRs for word 3 so that its single best-scoring codeword (per
        // `golay24_decode_soft`) is WRONG, but the true codeword is the runner-up --
        // then confirm `decode_header_soft`'s CRC-assisted list search still
        // recovers the true header (which a naive best-guess-only soft decoder
        // would not).
        let h = sample();
        let coded = encode_header(&h);
        let llrs_clean = strong_llrs(&coded);
        let mut deint = deinterleave_llrs(&llrs_clean);

        // Word 3's true (clean) codeword. LLR convention (matching
        // `golay24_decode_soft`'s scoring and `strong_llrs` above): a positive LLR
        // means bit 0, negative means bit 1.
        let mut true_cw: u32 = 0;
        for b in 0..24 {
            let bit = (deint[3 * 24 + b] < 0.0) as u32;
            true_cw = (true_cw << 1) | bit;
        }
        let (true_info, n_err) = golay24_decode(true_cw).expect("clean word must decode");
        assert_eq!(n_err, 0);

        // A minimum-weight (weight-8) nonzero codeword, XOR-ed into word 3's true
        // codeword, yields another valid codeword `wrong_cw` at exactly the Golay
        // code's minimum distance (8) from the truth -- the closest any other
        // codeword can ever get to `true_cw`.
        let delta_info = (1u16..4096)
            .find(|&i| golay24_encode(i).count_ones() == 8)
            .expect("a weight-8 codeword must exist in the (24,12) Golay code");
        let wrong_info = true_info ^ delta_info;
        let wrong_cw = golay24_encode(wrong_info);
        assert_eq!((true_cw ^ wrong_cw).count_ones(), 8);

        // Two-scale LLR construction (provably unambiguous, unlike a discrete
        // "flip k of the d differing bits" corruption, which -- right at this
        // code's very symmetric minimum-distance boundary -- often leaves 2+
        // codewords tied for second place; see this test's history/report for the
        // combinatorial search that confirmed this empirically):
        //   llr_b = A * sign_wrong(b) + eps * sign_true(b),  A >> eps > 0
        // The dominant `A` term makes `wrong_cw` the unique best: every other
        // codeword (including `true_cw`) is >= 8 (min distance) from `wrong_cw`, a
        // 16*A score gap that a 48*eps secondary term can't close. Among the
        // (large, symmetric) set of codewords exactly at that 8-away boundary from
        // `wrong_cw` -- which includes `true_cw` by construction -- the secondary
        // `eps` term alone decides second place, and it uniquely favors `true_cw`
        // (distance 0 to itself) over every other codeword in that tied set (which
        // are themselves each >= 8 from `true_cw`, by the same minimum-distance
        // property).
        const A: f32 = 4.0;
        const EPS: f32 = 0.1;
        let mut word3_llrs = [0.0f32; 24];
        for (b, slot) in word3_llrs.iter_mut().enumerate() {
            let wrong_bit = (wrong_cw >> (23 - b)) & 1;
            let true_bit = (true_cw >> (23 - b)) & 1;
            let sign_wrong = if wrong_bit == 0 { 1.0 } else { -1.0 };
            let sign_true = if true_bit == 0 { 1.0 } else { -1.0 };
            *slot = A * sign_wrong + EPS * sign_true;
        }

        // Confirm the setup actually produced a wrong best guess with the true
        // word as runner-up, i.e. this test is exercising the CRC list, not
        // coincidentally passing.
        let [(best, _), (second, _)] = golay24_decode_soft(&word3_llrs);
        assert_eq!(
            best, wrong_info,
            "test setup should make the wrong word score best"
        );
        assert_eq!(
            second, true_info,
            "test setup should keep the true word as runner-up"
        );

        deint[3 * 24..3 * 24 + 24].copy_from_slice(&word3_llrs);
        let llrs = interleave_llrs(&deint);

        assert_eq!(
            decode_header_soft(&llrs),
            Some(h),
            "CRC-assisted list decoding should rescue the true header even though \
             word 3's single best guess is wrong"
        );
    }
}
