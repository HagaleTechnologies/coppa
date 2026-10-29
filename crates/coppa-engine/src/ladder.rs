//! The speed ladder as a user sees it: one row per wire speed level (1-9) with
//! its modulation, code rate, maximum payload, and per-band air time and
//! bandwidth.
//!
//! This is the one source `coppa profiles` reads (COP-55), and the one a future
//! host display should read too. No number here is typed by hand: each comes
//! from the code the encoder runs -- `coppa_protocol::modem`'s
//! `code_rate_for_level`/`modulation_name_for_level`/`max_payload_for_level`/
//! `frame_airtime_s`, and this crate's own band and OFDM-profile selection
//! (`CoppaCore::level_allowed_on_band`/`select_ofdm_profile`). The tests below
//! check every row against real encoder output.
//!
//! Air time is for `CpMode::LongCp`, which is what the CLI transmits. Short CP
//! is a daemon-negotiated mode, off by default, and not shown here.

use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_protocol::cp_negotiator::CpMode;
use coppa_protocol::modem::{
    code_rate_for_level, frame_airtime_s, max_payload_for_level, modulation_name_for_level,
};

use crate::config::OfdmBand;
use crate::engine::CoppaCore;

/// What one speed level costs on one band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelOnBand {
    /// Occupied bandwidth of the band's OFDM profile, in Hz.
    pub bandwidth_hz: u32,
    /// Air time of one frame at this level on this band, in seconds.
    pub airtime_s: f64,
}

/// The user-facing facts of one wire speed level.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelInfo {
    /// Wire speed level (1-9).
    pub level: u8,
    /// Modulation name: "BPSK", "QPSK", "8PSK", "16-QAM" or "64-QAM".
    pub modulation: &'static str,
    /// LDPC code rate actually encoded, `(numerator, denominator)` in lowest terms.
    pub code_rate: (u16, u16),
    /// Largest payload one frame carries, in bytes (what `transmit` enforces).
    pub max_payload_bytes: usize,
    /// The level on the HF band, or `None` when HF does not carry it (level 9).
    pub hf: Option<LevelOnBand>,
    /// The level on the VHF band, which carries every level.
    pub vhf: LevelOnBand,
}

/// Every speed level, 1-9 in order.
pub fn speed_ladder() -> Vec<LevelInfo> {
    SPEED_LEVELS
        .iter()
        .filter_map(|speed| level_info(speed.level))
        .collect()
}

/// The facts for one speed level, or `None` outside 1-9.
pub fn level_info(level: u8) -> Option<LevelInfo> {
    let hf = if CoppaCore::level_allowed_on_band(level, OfdmBand::Hf) {
        Some(level_on_band(level, OfdmBand::Hf)?)
    } else {
        None
    };
    Some(LevelInfo {
        level,
        modulation: modulation_name_for_level(level)?,
        code_rate: code_rate_for_level(level)?,
        max_payload_bytes: max_payload_for_level(level)?,
        hf,
        vhf: level_on_band(level, OfdmBand::Vhf)?,
    })
}

fn level_on_band(level: u8, band: OfdmBand) -> Option<LevelOnBand> {
    let profile = CoppaCore::select_ofdm_profile(level, CpMode::LongCp, band);
    Some(LevelOnBand {
        bandwidth_hz: CoppaCore::profile_occupied_hz(&profile),
        airtime_s: frame_airtime_s(level, &profile)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EngineConfig;
    use crate::profiles::{HF_STANDARD, VHF_FAST};

    #[test]
    fn ladder_is_levels_1_to_9_without_gaps() {
        let levels: Vec<u8> = speed_ladder().iter().map(|row| row.level).collect();
        assert_eq!(levels, (1..=9).collect::<Vec<u8>>());
    }

    #[test]
    fn hf_carries_exactly_the_levels_an_hf_engine_accepts() {
        let ladder = speed_ladder();
        let hf_levels: Vec<u8> = ladder
            .iter()
            .filter(|row| row.hf.is_some())
            .map(|row| row.level)
            .collect();
        assert_eq!(
            hf_levels,
            CoppaCore::from_profile(&HF_STANDARD).available_speed_levels()
        );
        let all_levels: Vec<u8> = ladder.iter().map(|row| row.level).collect();
        assert_eq!(
            all_levels,
            CoppaCore::from_profile(&VHF_FAST).available_speed_levels()
        );
    }

    #[test]
    fn bandwidth_is_the_engines_own_figure() {
        let hf_hz = CoppaCore::from_profile(&HF_STANDARD).occupied_bandwidth_hz();
        let vhf_hz = CoppaCore::from_profile(&VHF_FAST).occupied_bandwidth_hz();
        assert_eq!(hf_hz, 2400);
        assert_eq!(vhf_hz, 5600);
        for row in speed_ladder() {
            if let Some(hf) = row.hf {
                assert_eq!(hf.bandwidth_hz, hf_hz, "level {}", row.level);
            }
            assert_eq!(row.vhf.bandwidth_hz, vhf_hz, "level {}", row.level);
        }
    }

    #[test]
    fn ladder_matches_what_the_encoder_does() {
        for band in [OfdmBand::Hf, OfdmBand::Vhf] {
            let core = CoppaCore::with_config(EngineConfig {
                ofdm_band: band,
                ..Default::default()
            });
            let sample_rate = core.config().sample_rate as f64;
            for row in speed_ladder() {
                let on_band = match band {
                    OfdmBand::Vhf => Some(row.vhf),
                    _ => row.hf,
                };
                match on_band {
                    Some(cell) => {
                        let max = vec![0x55; row.max_payload_bytes];
                        let samples =
                            core.encode_bytes_at_level(&max, row.level)
                                .unwrap_or_else(|e| {
                                    panic!("{band:?} level {}: max payload refused: {e}", row.level)
                                });
                        let airtime = samples.len() as f64 / sample_rate;
                        assert!(
                            (airtime - cell.airtime_s).abs() < 1e-9,
                            "{band:?} level {}: encoded {airtime} s, ladder says {} s",
                            row.level,
                            cell.airtime_s
                        );
                        let over = vec![0x55; row.max_payload_bytes + 1];
                        assert!(
                            core.encode_bytes_at_level(&over, row.level).is_err(),
                            "{band:?} level {}: max + 1 bytes accepted",
                            row.level
                        );
                    }
                    None => assert!(
                        core.encode_bytes_at_level(&[0x55], row.level).is_err(),
                        "{band:?} level {}: encoded on a band that does not carry it",
                        row.level
                    ),
                }
            }
        }
    }

    #[test]
    fn level_info_is_none_outside_1_to_9() {
        assert_eq!(level_info(0), None);
        assert_eq!(level_info(10), None);
    }
}
