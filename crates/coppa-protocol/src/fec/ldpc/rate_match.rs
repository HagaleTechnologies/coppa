//! Circular-buffer rate matching for the NR BG2 mother code.
//!
//! The mother code ([`super::nr_bg2`], Zc = 176) always encodes a fixed
//! `KB*ZC = 1760`-bit info block into an 8800-bit "mother codeword" (see
//! [`super::NrLdpc::encode`]): the systematic info bits with the first
//! `PUNCTURED_INFO_COLS*ZC = 352` bits (2 Zc-blocks) removed (they are never
//! transmitted, standard NR puncturing), followed by all 7392 parity bits.
//!
//! Every speed level additionally *shortens* that same mother code down to
//! its own `k_used` (fewer info bits actually carrying payload -- the tail
//! `k_used..1760` is known zero padding, not transmitted, and pinned back in
//! at RX instead -- see `CoppaTransceiver`'s pinning logic). Rate matching
//! selects exactly `E` coded bits (1944, this codec's fixed OFDM/interleaver
//! block size) from what's left, per 3GPP TS 38.212 §5.4.2's circular-buffer
//! procedure:
//!
//! ```text
//! buffer(k_used) = [ mother_info[0 .. k_used - 2*Zc] , mother_parity[..] ]
//! ```
//!
//! i.e. the transmitted (non-punctured, non-shortened) info prefix followed
//! by *all* parity bits (Phase 2 never further limits the buffer -- that
//! would only matter for very low code rates this ladder doesn't use). `E`
//! bits are then read circularly starting at `k0(rv)`.
//!
//! Phase 2 uses `rv = 0` (`k0 = 0`) exclusively -- all speed levels' actual
//! traffic is single-shot, not HARQ-IR. The `rv = 1..3` offsets are
//! implemented and tested now anyway so Phase 3 (incremental redundancy) is
//! pure plumbing on top of this, not a rate-matching redesign.

use super::nr_bg2::{KB, PUNCTURED_INFO_COLS, ZC};

/// Length of the non-punctured systematic info portion of the mother
/// codeword: `(KB - PUNCTURED_INFO_COLS) * ZC` = 1408.
const INFO_NONPUNCTURED_LEN: usize = (KB - PUNCTURED_INFO_COLS) * ZC;

/// Build the logical rate-matching buffer for a given `k_used`: the
/// transmitted info prefix (`mother[0..k_used-2*Zc]`) followed by all of the
/// mother codeword's parity bits (`mother[INFO_NONPUNCTURED_LEN..]`).
///
/// # Panics
/// Panics if `k_used < PUNCTURED_INFO_COLS*ZC` (would make the transmitted
/// info prefix length negative) or `k_used > KB*ZC` (more info than the
/// mother code has), or if `mother.len() < INFO_NONPUNCTURED_LEN` (mother
/// codeword too short to have been produced by `NrLdpc::encode`).
fn matching_buffer<T: Copy>(mother: &[T], k_used: usize) -> Vec<T> {
    assert!(
        k_used >= PUNCTURED_INFO_COLS * ZC,
        "k_used={k_used} must be >= {} (2*Zc, the punctured prefix)",
        PUNCTURED_INFO_COLS * ZC
    );
    assert!(
        k_used <= KB * ZC,
        "k_used={k_used} must be <= {} (KB*ZC, the mother code's info width)",
        KB * ZC
    );
    assert!(
        mother.len() >= INFO_NONPUNCTURED_LEN,
        "mother.len()={} shorter than the non-punctured info region ({})",
        mother.len(),
        INFO_NONPUNCTURED_LEN
    );

    let info_len = k_used - PUNCTURED_INFO_COLS * ZC;
    let mut buf = Vec::with_capacity(info_len + (mother.len() - INFO_NONPUNCTURED_LEN));
    buf.extend_from_slice(&mother[..info_len]);
    buf.extend_from_slice(&mother[INFO_NONPUNCTURED_LEN..]);
    buf
}

/// Redundancy-version starting offset into the rate-matching buffer, per
/// 3GPP's `k0 = [0, buf/4, buf/2, 3*buf/4]` (rounded down to a multiple of
/// `Zc`, since a QC-LDPC circular buffer read must start on a lifted-block
/// boundary to keep the code's quasi-cyclic structure meaningful).
///
/// # Panics
/// Panics if `rv > 3`.
fn k0_offset(buf_len: usize, rv: u8) -> usize {
    let raw = match rv {
        0 => 0,
        1 => buf_len / 4,
        2 => buf_len / 2,
        3 => 3 * buf_len / 4,
        _ => panic!("invalid redundancy version rv={rv}, must be 0..=3"),
    };
    (raw / ZC) * ZC
}

/// Select `e` coded bits from the mother codeword (circular buffer, per
/// speed level's `k_used`). RV0 (`k0=0`) is Phase 2's only actual use; RV1-3
/// are implemented for Phase 3 (HARQ incremental redundancy).
///
/// # Panics
/// See [`matching_buffer`] and [`k0_offset`].
pub fn rate_match(mother: &[u8], k_used: usize, e: usize, rv: u8) -> Vec<u8> {
    let buf = matching_buffer(mother, k_used);
    let buf_len = buf.len();
    let k0 = k0_offset(buf_len, rv);
    (0..e).map(|i| buf[(k0 + i) % buf_len]).collect()
}

/// Inverse of [`rate_match`]: scatter `E` received LLRs back into a
/// mother-length LLR buffer. Positions never observed (including the
/// shortened tail `k_used..KB*ZC`, which this function deliberately leaves
/// at `0.0` -- see module docs) are `0.0` ("no information"); the caller is
/// responsible for pinning known-zero shortened positions to a confident
/// LLR afterward (see `CoppaTransceiver::receive_with_metrics`'s pinning
/// block), and `NrLdpc::decode_soft` is responsible for prepending the
/// always-punctured leading `2*Zc` positions (never part of `mother_len`
/// here -- see that function's docs).
///
/// If `e > buf_len` (circular wraparound covers a position more than once --
/// never happens in Phase 2, where `e=1944 <<` every level's buffer length,
/// but is possible in principle for Phase 3 IR combining across
/// retransmissions), repeated observations of the same position are summed
/// (soft/LLR chase-combining), not overwritten.
///
/// # Panics
/// Panics if `llrs.len() != e`, or (via [`matching_buffer`]/[`k0_offset`])
/// for invalid `k_used`/`rv`/`mother_len`.
pub fn rate_dematch(llrs: &[f32], k_used: usize, e: usize, rv: u8, mother_len: usize) -> Vec<f32> {
    assert_eq!(llrs.len(), e, "llrs.len() must equal e");

    let info_len = k_used - PUNCTURED_INFO_COLS * ZC;
    let parity_len = mother_len - INFO_NONPUNCTURED_LEN;
    let buf_len = info_len + parity_len;
    let k0 = k0_offset(buf_len, rv);

    let mut buf = vec![0.0f32; buf_len];
    for (i, &llr) in llrs.iter().enumerate() {
        let pos = (k0 + i) % buf_len;
        buf[pos] += llr;
    }

    let mut out = vec![0.0f32; mother_len];
    out[..info_len].copy_from_slice(&buf[..info_len]);
    out[INFO_NONPUNCTURED_LEN..].copy_from_slice(&buf[info_len..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `k_used` for wire speed levels 1..10 (level 8 reserved), per the
    /// audited ladder (Task 4 Step 4): `{486, 972, 972, 1458, 1296, 972,
    /// 1458, 1296, 1620}` for levels `{1,2,3,4,5,6,7,9,10}`.
    const ALL_K_USED: [usize; 9] = [486, 972, 972, 1458, 1296, 972, 1458, 1296, 1620];

    const E: usize = 1944;
    const MOTHER_LEN: usize = 8800;

    fn dummy_mother() -> Vec<u8> {
        (0..MOTHER_LEN).map(|i| (i % 2) as u8).collect()
    }

    #[test]
    fn rate_match_output_length_is_always_e_for_all_levels_and_rvs() {
        let mother = dummy_mother();
        for &k_used in &ALL_K_USED {
            for rv in 0..=3u8 {
                let out = rate_match(&mother, k_used, E, rv);
                assert_eq!(
                    out.len(),
                    E,
                    "k_used={k_used} rv={rv}: expected length {E}, got {}",
                    out.len()
                );
            }
        }
    }

    #[test]
    fn rate_dematch_output_length_is_always_mother_len() {
        for &k_used in &ALL_K_USED {
            for rv in 0..=3u8 {
                let llrs = vec![0.3f32; E];
                let out = rate_dematch(&llrs, k_used, E, rv, MOTHER_LEN);
                assert_eq!(out.len(), MOTHER_LEN);
            }
        }
    }

    #[test]
    fn rv0_k0_is_always_zero() {
        let mother = dummy_mother();
        for &k_used in &ALL_K_USED {
            let out = rate_match(&mother, k_used, E, 0);
            let buf = matching_buffer(&mother, k_used);
            assert_eq!(&out[..], &buf[..E], "rv0 must read from buffer offset 0");
        }
    }

    #[test]
    fn k0_offsets_are_zc_multiples_and_monotonic() {
        for &k_used in &ALL_K_USED {
            let info_len = k_used - PUNCTURED_INFO_COLS * ZC;
            let buf_len = info_len + (MOTHER_LEN - INFO_NONPUNCTURED_LEN);
            let mut prev = 0usize;
            for rv in 0..=3u8 {
                let k0 = k0_offset(buf_len, rv);
                assert_eq!(k0 % ZC, 0, "k0 must be a Zc multiple, got {k0}");
                assert!(
                    k0 < buf_len,
                    "k0={k0} must be within the buffer ({buf_len})"
                );
                if rv > 0 {
                    assert!(k0 >= prev, "k0 offsets should be non-decreasing with rv");
                }
                prev = k0;
            }
        }
    }

    #[test]
    fn round_trip_recovers_transmitted_bits_as_confident_llrs() {
        // Encode a known bit pattern into "soft" LLRs (+1/-1 => strong
        // confidence), rate_match -> rate_dematch, and check that every
        // position that was actually transmitted decodes back to the
        // correct sign, while untransmitted positions stay exactly 0.0.
        let mother = dummy_mother();
        for &k_used in &ALL_K_USED {
            let matched = rate_match(&mother, k_used, E, 0);
            let llrs: Vec<f32> = matched
                .iter()
                .map(|&b| if b == 0 { 3.0 } else { -3.0 })
                .collect();
            let dematched = rate_dematch(&llrs, k_used, E, 0, MOTHER_LEN);

            let info_len = k_used - PUNCTURED_INFO_COLS * ZC;
            // Transmitted info prefix (rv0, first `info_len` of E were read
            // from buffer offset 0..info_len, i.e. exactly this range).
            for (i, &llr) in dematched.iter().enumerate().take(info_len.min(E)) {
                let expected_bit = mother[i];
                let sign_ok = if expected_bit == 0 {
                    llr > 0.0
                } else {
                    llr < 0.0
                };
                assert!(sign_ok, "k_used={k_used} pos={i}: dematched sign mismatch");
            }
            // Shortened tail (k_used-2Zc .. 1408) must remain exactly 0.0 --
            // rate_dematch never touches it; it's the caller's to pin.
            for &llr in &dematched[info_len..INFO_NONPUNCTURED_LEN] {
                assert_eq!(
                    llr, 0.0,
                    "k_used={k_used}: shortened tail must be left at 0.0"
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "invalid redundancy version")]
    fn rejects_invalid_rv() {
        let mother = dummy_mother();
        rate_match(&mother, 1620, E, 4);
    }

    #[test]
    #[should_panic(expected = "k_used")]
    fn rejects_k_used_below_punctured_width() {
        let mother = dummy_mother();
        rate_match(&mother, 100, E, 0);
    }

    #[test]
    #[should_panic(expected = "k_used")]
    fn rejects_k_used_above_kb_width() {
        let mother = dummy_mother();
        rate_match(&mother, KB * ZC + 1, E, 0);
    }
}
