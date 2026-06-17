use crate::fec::ldpc::codes::CodeRate;
use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_codec::traits::ConstellationMapper;

/// Returns (ConstellationMapper, CodeRate) for a wire-encoded speed level (1-10, 8 reserved).
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
        9 => Ok((Box::new(Qam64Mapper), CodeRate::Rate2_3)),
        10 => Ok((Box::new(Qam64Mapper), CodeRate::Rate7_8)),
        8 => Err("Speed level 8 (32-QAM) is reserved".into()),
        _ => Err(format!("Invalid speed level: {}", wire_level)),
    }
}

/// Look up the SPEED_LEVELS entry by wire-encoded level number.
pub fn speed_level_entry(
    wire_level: u8,
) -> Option<&'static coppa_codec::ofdm::coppa_modem::SpeedLevel> {
    SPEED_LEVELS.iter().find(|sl| sl.level == wire_level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_valid_levels_return_ok() {
        for wire_level in [1, 2, 3, 4, 5, 6, 7, 9, 10] {
            let result = speed_level_components(wire_level);
            assert!(result.is_ok(), "Level {} should be valid", wire_level);
        }
    }

    #[test]
    fn test_reserved_level_8_returns_err() {
        let result = speed_level_components(8);
        assert!(result.is_err(), "Level 8 (32-QAM) should be reserved");
    }

    #[test]
    fn test_invalid_levels_return_err() {
        for wire_level in [0, 11, 15, 255] {
            let result = speed_level_components(wire_level);
            assert!(result.is_err(), "Level {} should be invalid", wire_level);
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
            (9, 6),
            (10, 6), // 64QAM
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
            (9, CodeRate::Rate2_3),
            (10, CodeRate::Rate7_8),
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
}
