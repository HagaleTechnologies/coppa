use crate::fec::ldpc::codes::CodeRate;
use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_codec::traits::ConstellationMapper;

/// Returns (ConstellationMapper, CodeRate) for a wire-encoded speed level (1-9).
pub fn speed_level_components(
    wire_level: u8,
) -> Result<(Box<dyn ConstellationMapper>, CodeRate), String> {
    use coppa_codec::bpsk::BpskMapper;
    use coppa_codec::psk8::Psk8Mapper;
    use coppa_codec::qam16::Qam16Mapper;
    use coppa_codec::qam64::Qam64Mapper;
    use coppa_codec::qpsk::QpskMapper;

    match wire_level {
        1 => Ok((Box::new(BpskMapper), CodeRate::Rate1_4)),
        2 => Ok((Box::new(BpskMapper), CodeRate::Rate1_2)),
        3 => Ok((Box::new(QpskMapper), CodeRate::Rate1_2)),
        4 => Ok((Box::new(QpskMapper), CodeRate::Rate3_4)),
        5 => Ok((Box::new(Psk8Mapper), CodeRate::Rate2_3)),
        6 => Ok((Box::new(Qam16Mapper), CodeRate::Rate1_2)),
        7 => Ok((Box::new(Qam16Mapper), CodeRate::Rate3_4)),
        8 => Ok((Box::new(Qam64Mapper), CodeRate::Rate2_3)),
        9 => Ok((Box::new(Qam64Mapper), CodeRate::Rate7_8)),
        _ => Err(format!("Invalid speed level: {}", wire_level)),
    }
}

/// Look up the SPEED_LEVELS entry by wire-encoded level number.
pub fn speed_level_entry(
    wire_level: u8,
) -> Option<&'static coppa_codec::ofdm::coppa_modem::SpeedLevel> {
    SPEED_LEVELS.iter().find(|sl| sl.level == wire_level)
}

/// `k_used` (shortened NR BG2 mother-code info length) per wire-encoded
/// speed level, Task 4 of the Phase 2 remediation roadmap.
///
/// Coppa now uses **one** LDPC mother code ([`crate::fec::ldpc::NrLdpc`],
/// Zc=176 BG2, fixed `KB*ZC = 1760`-bit info width) for every speed level,
/// instead of switching between nine different per-rate base matrices. Each
/// level's nominal code rate is instead realized by *shortening*: only the
/// first `k_used` of the 1760 systematic info bits actually carry payload
/// (plus zero-pad up to `k_used`); the remaining `1760 - k_used` are known
/// zero-pad, never transmitted, and pinned back in at RX (see
/// `CoppaTransceiver::receive_with_metrics`'s pinning block). `rate_match`
/// then selects exactly 1944 coded bits from the (rate-matched) mother
/// codeword, matching this codec's fixed OFDM/interleaver block size.
///
/// `k_used = round(rate * 1944)` for this ladder's existing rates, with one
/// **wire-format-breaking exception**: level 9 (64-QAM) changes from rate
/// 7/8 (1701) to rate **5/6** (1620), per the Phase 2 decision audit ("k_used
/// table" decision) -- 7/8 was found to hit LDPC non-convergence at high SNR
/// (see `CLAUDE.md`'s Known Limitations), and 5/6 is both a real reduction
/// in that failure mode and a cleaner NR-standard-adjacent rate. **Frames
/// encoded with the pre-Task-4 codec are not decodable by this codec, and
/// vice versa** -- this is a wire-format break exactly like the Phase 1 OFDM
/// waveform break (see `docs/adr/003-phase1-waveform-break.md` for the
/// pattern, and this task's own ADR for this specific change).
pub fn k_used_for_level(wire_level: u8) -> Option<usize> {
    match wire_level {
        1 => Some(486),  // BPSK, rate 1/4
        2 => Some(972),  // BPSK, rate 1/2
        3 => Some(972),  // QPSK, rate 1/2
        4 => Some(1458), // QPSK, rate 3/4
        5 => Some(1296), // 8PSK, rate 2/3
        6 => Some(972),  // 16QAM, rate 1/2
        7 => Some(1458), // 16QAM, rate 3/4
        8 => Some(1296), // 64QAM, rate 2/3
        9 => Some(1620), // 64QAM, rate 5/6 (was 7/8 pre-Task-4 -- wire break, see doc above)
        _ => None,
    }
}

/// Maximum application-payload size (bytes) `CoppaTransceiver::transmit` will accept
/// for a given wire-encoded speed level (Phase 3 Task 1: payload CRC-32 + hard
/// oversize rejection).
///
/// `CoppaTransceiver::transmit` appends a CRC-32 (4 bytes) to the payload before
/// scrambling/padding into this level's shortened `k_used`-bit NR BG2 info block
/// (see `k_used_for_level`'s doc), so the real per-level payload budget is
/// `PAYLOAD_CRC_LEN` (4) bytes less than the raw `k_used/8` byte capacity:
/// `max_payload = k_used/8 - PAYLOAD_CRC_LEN` (integer division). Returns `None`
/// for invalid levels, mirroring `k_used_for_level`.
pub fn max_payload_for_level(wire_level: u8) -> Option<usize> {
    use crate::modem::transceiver::PAYLOAD_CRC_LEN;
    k_used_for_level(wire_level).map(|k_used| k_used / 8 - PAYLOAD_CRC_LEN)
}

/// Nominal LDPC code rate of a wire-encoded speed level, as `(numerator,
/// denominator)` in lowest terms: `k_used_for_level(level) / CODED_BLOCK_LEN`
/// (1944). Returns `None` for an invalid level.
///
/// This is the rate the encoder actually runs. Read it here rather than from
/// `speed_level_components`'s `CodeRate` or `SPEED_LEVELS[8].ldpc_rate_num/den`
/// (`coppa_codec::ofdm::coppa_modem`): both still say 7/8 for level 9, which
/// `k_used` moved to 5/6 (see `docs/SPEC.md` §12). `coppa profiles` shows this
/// value.
pub fn code_rate_for_level(wire_level: u8) -> Option<(u16, u16)> {
    use crate::modem::transceiver::CODED_BLOCK_LEN;
    fn gcd(a: usize, b: usize) -> usize {
        if b == 0 {
            a
        } else {
            gcd(b, a % b)
        }
    }
    let k_used = k_used_for_level(wire_level)?;
    let d = gcd(k_used, CODED_BLOCK_LEN);
    Some(((k_used / d) as u16, (CODED_BLOCK_LEN / d) as u16))
}

/// User-facing modulation name of a wire-encoded speed level ("BPSK", "QPSK",
/// "8PSK", "16-QAM" or "64-QAM"), derived from the constellation mapper
/// `speed_level_components` returns. Returns `None` for an invalid level.
pub fn modulation_name_for_level(wire_level: u8) -> Option<&'static str> {
    let (mapper, _) = speed_level_components(wire_level).ok()?;
    match mapper.bits_per_symbol() {
        1 => Some("BPSK"),
        2 => Some("QPSK"),
        3 => Some("8PSK"),
        4 => Some("16-QAM"),
        6 => Some("64-QAM"),
        _ => None,
    }
}

/// Maximum TOTAL application-payload size (bytes) a multi-codeword frame
/// (Phase 3 Task 5) will accept for a given wire-encoded speed level and
/// codeword count: `codewords` independent LDPC codewords, each with its own
/// CRC-32 trailer, so the total budget is simply `codewords *
/// max_payload_for_level(level)` -- see
/// `crate::modem::transceiver::split_payload_across_codewords` for how a
/// payload at or under this cap is guaranteed to split into per-codeword
/// chunks that each individually fit `max_payload_for_level(level)` too.
/// Returns `None` for invalid levels, mirroring `max_payload_for_level`.
pub fn max_multi_payload_for_level(wire_level: u8, codewords: u8) -> Option<usize> {
    max_payload_for_level(wire_level).map(|per_cw| per_cw * codewords.max(1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k_used_matches_audited_ladder() {
        let cases = [
            (1, 486),
            (2, 972),
            (3, 972),
            (4, 1458),
            (5, 1296),
            (6, 972),
            (7, 1458),
            (8, 1296),
            (9, 1620),
        ];
        for (level, expected) in cases {
            assert_eq!(
                k_used_for_level(level),
                Some(expected),
                "level {level}: k_used mismatch"
            );
        }
    }

    #[test]
    fn max_payload_matches_k_used_minus_crc_trailer() {
        // max_payload = k_used/8 - 4 (integer division), per this level's k_used.
        let cases = [
            (1, 56),  // 486/8=60 (floor) - 4
            (2, 117), // 972/8=121 - 4
            (3, 117), // 972/8=121 - 4
            (4, 178), // 1458/8=182 - 4
            (5, 158), // 1296/8=162 - 4
            (6, 117), // 972/8=121 - 4
            (7, 178), // 1458/8=182 - 4
            (8, 158), // 1296/8=162 - 4
            (9, 198), // 1620/8=202 - 4
        ];
        for (level, expected) in cases {
            assert_eq!(
                max_payload_for_level(level),
                Some(expected),
                "level {level}: max_payload mismatch"
            );
        }
    }

    #[test]
    fn max_payload_invalid_levels_are_none() {
        for level in [0, 10, 11, 15, 255] {
            assert_eq!(
                max_payload_for_level(level),
                None,
                "level {level} should be None"
            );
        }
    }

    #[test]
    fn k_used_invalid_levels_are_none() {
        for level in [0, 10, 11, 15, 255] {
            assert_eq!(
                k_used_for_level(level),
                None,
                "level {level} should be None"
            );
        }
    }

    #[test]
    fn k_used_within_mother_code_bounds() {
        // Every k_used must be >= 2*Zc (the punctured prefix) and <= KB*Zc
        // (the mother code's full info width) -- see rate_match.rs.
        use crate::fec::ldpc::nr_bg2::{KB, PUNCTURED_INFO_COLS, ZC};
        for level in [1, 2, 3, 4, 5, 6, 7, 8, 9] {
            let k = k_used_for_level(level).unwrap();
            assert!(
                k >= PUNCTURED_INFO_COLS * ZC,
                "level {level}: k_used={k} too small"
            );
            assert!(k <= KB * ZC, "level {level}: k_used={k} too large");
        }
    }

    #[test]
    fn test_all_valid_levels_return_ok() {
        for wire_level in [1, 2, 3, 4, 5, 6, 7, 8, 9] {
            let result = speed_level_components(wire_level);
            assert!(result.is_ok(), "Level {} should be valid", wire_level);
        }
    }

    #[test]
    fn test_level_8_is_64qam_rate_2_3() {
        let (mapper, rate) = speed_level_components(8).expect("level 8 must be valid");
        assert_eq!(mapper.bits_per_symbol(), 6);
        assert_eq!(rate, CodeRate::Rate2_3);
    }

    #[test]
    fn test_invalid_levels_return_err() {
        for wire_level in [0, 10, 11, 15, 255] {
            let result = speed_level_components(wire_level);
            assert_eq!(
                result.err(),
                Some(format!("Invalid speed level: {wire_level}"))
            );
        }
    }

    #[test]
    fn test_bits_per_symbol_matches() {
        let cases = [
            (1, 1),
            (2, 1), // BPSK
            (3, 2),
            (4, 2), // QPSK
            (5, 3), // 8PSK
            (6, 4),
            (7, 4), // 16QAM
            (8, 6),
            (9, 6), // 64QAM
        ];
        for (wire_level, expected_bps) in cases {
            let (mapper, _) = speed_level_components(wire_level).unwrap();
            assert_eq!(
                mapper.bits_per_symbol(),
                expected_bps,
                "Level {} should have {} bits/sym",
                wire_level,
                expected_bps
            );
        }
    }

    #[test]
    fn test_code_rates_match() {
        let cases = [
            (1, CodeRate::Rate1_4),
            (2, CodeRate::Rate1_2),
            (3, CodeRate::Rate1_2),
            (4, CodeRate::Rate3_4),
            (5, CodeRate::Rate2_3),
            (6, CodeRate::Rate1_2),
            (7, CodeRate::Rate3_4),
            (8, CodeRate::Rate2_3),
            (9, CodeRate::Rate7_8),
        ];
        for (wire_level, expected_rate) in cases {
            let (_, rate) = speed_level_components(wire_level).unwrap();
            assert_eq!(
                rate, expected_rate,
                "Level {} should have rate {:?}",
                wire_level, expected_rate
            );
        }
    }

    #[test]
    fn code_rate_is_k_used_over_1944_in_lowest_terms() {
        let expected = [
            (1, (1, 4)),
            (2, (1, 2)),
            (3, (1, 2)),
            (4, (3, 4)),
            (5, (2, 3)),
            (6, (1, 2)),
            (7, (3, 4)),
            (8, (2, 3)),
            (9, (5, 6)),
        ];
        for (level, rate) in expected {
            assert_eq!(code_rate_for_level(level), Some(rate), "level {level}");
        }
        // Not the stale CodeRate::Rate7_8 label level 9 still carries.
        assert_ne!(code_rate_for_level(9), Some((7, 8)));
    }

    #[test]
    fn code_rate_and_modulation_are_none_outside_1_to_9() {
        for level in [0, 10] {
            assert_eq!(code_rate_for_level(level), None, "level {level}");
            assert_eq!(modulation_name_for_level(level), None, "level {level}");
        }
    }

    #[test]
    fn modulation_names_match_spec_section_6() {
        let expected = [
            "BPSK", "BPSK", "QPSK", "QPSK", "8PSK", "16-QAM", "16-QAM", "64-QAM", "64-QAM",
        ];
        for (i, name) in expected.iter().enumerate() {
            let level = i as u8 + 1;
            assert_eq!(
                modulation_name_for_level(level),
                Some(*name),
                "level {level}"
            );
        }
    }

    #[test]
    fn modulation_mapper_agrees_with_speed_levels_table() {
        for entry in SPEED_LEVELS.iter() {
            let (mapper, _) = speed_level_components(entry.level).unwrap();
            assert_eq!(
                mapper.bits_per_symbol(),
                entry.bits_per_symbol as usize,
                "level {}",
                entry.level
            );
        }
    }
}
