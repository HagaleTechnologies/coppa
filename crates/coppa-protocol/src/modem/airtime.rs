//! Frame airtime calculation for half-duplex ARQ timing.
//!
//! Phase 3 Task 2 (decision 4, "Half-duplex ARQ discipline") needs a *computed*
//! RTO floor: `rto_floor = burst_airtime(window, level) + 2*turnaround +
//! ack_airtime(level_ack)`. Both `burst_airtime` and `ack_airtime` reduce to "how
//! long does ONE OFDM frame at this speed level take to transmit" -- this module
//! provides that primitive ([`frame_airtime_s`]), computed from the real
//! `SPEED_LEVELS` table and `CoppaProfile` geometry rather than guessed.
//!
//! See [`crate::arq::rto_floor`] for the RTO-floor formula that consumes this.

use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_codec::ofdm::header_fec::PROTECTED_HEADER_CODED_BITS;
use coppa_codec::ofdm::pilots::CoppaPilotPattern;
use coppa_codec::ofdm::CoppaProfile;

/// Fixed per-frame LDPC coded-block length (1944 coded bits), matching
/// `CoppaModem::demodulate_frame`'s own `CODED_BLOCK_LEN`. Duplicated here as a
/// small local constant rather than plumbed through from `coppa-codec`, following
/// the same established pattern as the identically-named, independently
/// duplicated constants in `crate::modem::transceiver` and `crate::modem::streaming`
/// (see those modules' own `CODED_BLOCK_LEN` doc comments) -- this value is fixed
/// by the OFDM/interleaver block size and does not vary per call site.
const CODED_BLOCK_LEN: usize = 1944;

/// Number of OFDM symbols transmitted before the first header symbol: 2 preamble
/// symbols + 1 full-comb probe symbol. Mirrors `CoppaModem::modulate_mapped`'s
/// steps 1-2 and `demodulate_frame`'s `data_start = timing_offset + 3 * symbol_len`.
const PREAMBLE_SYMS: usize = 3;

/// Number of usable data (non-pilot) OFDM carriers per symbol for `profile`.
///
/// Mirrors `CoppaModem::data_carriers_per_symbol`'s own `self.pilots.num_data()`
/// without needing a full `CoppaModem` instance (which also builds an FFT plan and
/// a TX bandpass filter this calculation doesn't need).
fn data_carriers_per_symbol(profile: &CoppaProfile) -> usize {
    CoppaPilotPattern::new(profile.total_active_carriers(), profile.pilot_carriers).num_data()
}

/// Airtime, in seconds, to transmit ONE full OFDM frame at wire-encoded speed
/// `level` using `profile`. Returns `None` for a reserved/invalid level (e.g. 8,
/// or anything not present in `SPEED_LEVELS`), or if `profile` has zero data
/// carriers (degenerate profile).
///
/// Every frame at a given level costs the same airtime regardless of the actual
/// payload size it carries: `CoppaTransceiver` always rate-matches to a fixed
/// `CODED_BLOCK_LEN`-bit codeword (smaller payloads are zero-padded, not
/// shortened -- see `crate::modem::transceiver::CODED_BLOCK_LEN`'s doc), so this
/// one function covers both real data frames and small ACK frames sent at the
/// same level, which is exactly what [`crate::arq::rto_floor`]'s
/// `burst_airtime`/`ack_airtime` terms need.
///
/// `frame_airtime_s = (PREAMBLE_SYMS + header_syms + payload_syms) * symbol_len /
/// sample_rate`, mirroring `CoppaModem::demodulate_frame`'s own symbol-count
/// arithmetic (see that method's `num_header_syms` / `coded_symbols` /
/// `num_payload_syms`, `crates/coppa-codec/src/ofdm/coppa_modem.rs`).
pub fn frame_airtime_s(level: u8, profile: &CoppaProfile) -> Option<f64> {
    let sl = SPEED_LEVELS.iter().find(|s| s.level == level)?;
    let data_per_sym = data_carriers_per_symbol(profile);
    if data_per_sym == 0 {
        return None;
    }
    let symbol_len = profile.fft_size + profile.cp_samples;

    let header_syms = PROTECTED_HEADER_CODED_BITS.div_ceil(data_per_sym);
    let coded_symbols = CODED_BLOCK_LEN.div_ceil(sl.bits_per_symbol as usize);
    let payload_syms = coded_symbols.div_ceil(data_per_sym);

    let total_syms = PREAMBLE_SYMS + header_syms + payload_syms;
    Some((total_syms * symbol_len) as f64 / profile.sample_rate as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_airtime_level2_hf_standard_matches_hand_calc() {
        // level 2 (BPSK, rate 1/2): data_per_sym = 44 (48 active - 4 pilots).
        // header_syms = ceil(144/44) = 4. coded_symbols = ceil(1944/1) = 1944.
        // payload_syms = ceil(1944/44) = 45. total = 3 + 4 + 45 = 52 symbols.
        // symbol_len = 960 + 300 = 1260. airtime = 52*1260/48000 = 1.365s.
        let profile = CoppaProfile::hf_standard();
        let t = frame_airtime_s(2, &profile).expect("level 2 is valid");
        assert!((t - 1.365).abs() < 1e-9, "expected ~1.365s, got {t}");
    }

    #[test]
    fn frame_airtime_higher_order_modulation_is_shorter() {
        // Same LDPC block, more bits/symbol -> fewer OFDM symbols -> less airtime.
        let profile = CoppaProfile::hf_standard();
        let t2 = frame_airtime_s(2, &profile).unwrap(); // BPSK
        let t10 = frame_airtime_s(10, &profile).unwrap(); // 64-QAM
        assert!(
            t10 < t2,
            "64-QAM frame ({t10}s) should be shorter than BPSK ({t2}s)"
        );
    }

    #[test]
    fn frame_airtime_reserved_level_is_none() {
        let profile = CoppaProfile::hf_standard();
        assert!(frame_airtime_s(8, &profile).is_none());
    }

    #[test]
    fn frame_airtime_invalid_level_is_none() {
        let profile = CoppaProfile::hf_standard();
        assert!(frame_airtime_s(255, &profile).is_none());
    }

    /// COP-2: `hf_standard` and `hf_standard_short_cp` differ in exactly one field
    /// that airtime depends on (`cp_samples`), and `total_syms` depends on the
    /// profile only through `data_carriers_per_symbol` -- identical for both (44
    /// data / 4 pilot). So the short-CP airtime is the long-CP airtime scaled by
    /// `(960+144)/(960+300) = 1104/1260`, THE SAME CONSTANT AT EVERY LEVEL.
    ///
    /// This is what makes an airtime-normalized *ratio* (adaptive/best-fixed,
    /// adaptive/oracle -- ADR-008's bar) invariant to a uniform CP change: the
    /// constant cancels. Any movement in those ratios under short CP is therefore a
    /// FER or rate-trajectory effect, never the airtime saving itself.
    #[test]
    fn short_cp_scales_frame_airtime_by_one_constant_at_every_level() {
        let long = CoppaProfile::hf_standard();
        let short = CoppaProfile::hf_standard_short_cp();
        let expected = 1104.0 / 1260.0;
        for level in [1u8, 2, 3, 4, 5, 6, 7, 9, 10] {
            let l = frame_airtime_s(level, &long).expect("valid level");
            let s = frame_airtime_s(level, &short).expect("valid level");
            assert!(
                ((s / l) - expected).abs() < 1e-12,
                "level {level}: short/long = {}, expected {expected}",
                s / l
            );
        }
    }

    /// The factorization the test above rests on: airtime is
    /// `total_syms(level, profile) * symbol_len(profile) / sample_rate`, and the two
    /// profiles share `total_syms`. Pinned separately so a future change to
    /// `data_carriers`/`pilot_carriers` on either profile fails HERE, with a clear
    /// message, rather than silently invalidating the invariance argument.
    #[test]
    fn short_cp_and_long_cp_share_the_same_symbol_count_per_level() {
        let long = CoppaProfile::hf_standard();
        let short = CoppaProfile::hf_standard_short_cp();
        assert_eq!(
            data_carriers_per_symbol(&long),
            data_carriers_per_symbol(&short)
        );
        for level in [1u8, 2, 3, 4, 5, 6, 7, 9, 10] {
            let syms_long = frame_airtime_s(level, &long).unwrap() * 48_000.0 / 1260.0;
            let syms_short = frame_airtime_s(level, &short).unwrap() * 48_000.0 / 1104.0;
            assert!((syms_long - syms_short).abs() < 1e-9, "level {level}");
        }
    }
}
