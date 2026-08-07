//! Reusable helpers for the per-frame pre-FEC link diagnosis.

use coppa_codec::ofdm::coppa_modem::CoppaModem;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::ofdm::CoppaProfile;
use coppa_codec::traits::ConstellationMapper;
use coppa_protocol::fec::ldpc::{rate_match, NrLdpc};
use coppa_protocol::fec::scrambler::scramble;
use coppa_protocol::modem::speed_levels::{speed_level_components, speed_level_entry};
use crc::{Crc, CRC_32_ISO_HDLC};
use num_complex::Complex32;

pub const CODED_BLOCK_LEN: usize = 1944;

pub struct DiagnosticFrame {
    pub signal: Vec<f32>,
    pub interleaved_bits: Vec<u8>,
}

pub fn symbols_needed(bits_per_symbol: usize) -> usize {
    CODED_BLOCK_LEN.div_ceil(bits_per_symbol)
}

pub fn hard_decide(mapper: &dyn ConstellationMapper, symbols: &[Complex32]) -> Vec<u8> {
    symbols
        .iter()
        .flat_map(|&symbol| mapper.demap_hard(symbol))
        .take(CODED_BLOCK_LEN)
        .collect()
}

pub fn build_diagnostic_frame(
    profile: &CoppaProfile,
    level: u8,
    payload: &[u8],
) -> DiagnosticFrame {
    let modem = CoppaModem::new(profile.clone(), 1);
    let (mapper, code_rate) = speed_level_components(level).expect("valid speed level");
    let checksum = Crc::<u32>::new(&CRC_32_ISO_HDLC).checksum(payload);
    let mut payload_with_crc = Vec::with_capacity(payload.len() + 4);
    payload_with_crc.extend_from_slice(payload);
    payload_with_crc.extend_from_slice(&checksum.to_be_bytes());
    let mut bits = Vec::with_capacity(NrLdpc::INFO_LEN);
    for &byte in &payload_with_crc {
        for shift in (0..8).rev() {
            bits.push((byte >> shift) & 1);
        }
    }
    bits.resize(code_rate.info_bits(), 0);
    bits.resize(NrLdpc::INFO_LEN, 0);
    scramble(&mut bits);
    let mother = NrLdpc::new().encode(&bits);
    let coded = rate_match::rate_match(&mother, code_rate.info_bits(), CODED_BLOCK_LEN, 0);
    let interleaved_bits =
        BlockInterleaver::new(CODED_BLOCK_LEN, profile.data_carriers).interleave(&coded);
    let symbols = mapper.map_bits(&interleaved_bits);
    let entry = speed_level_entry(level).expect("valid speed level");
    let header = CoppaHeader {
        version: 1,
        phy_mode: profile.phy_mode,
        frame_type: CoppaFrameType::Data,
        bandwidth: profile.bandwidth_id,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len: payload.len() as u16,
        codewords: 1,
    };
    let signal = modem.modulate_mapped(&header, &symbols, entry.papr_target_db);
    DiagnosticFrame {
        signal,
        interleaved_bits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_protocol::modem::transceiver::CoppaTransceiver;

    fn pseudorandom_bits(len: usize) -> Vec<u8> {
        (0..len)
            .scan(0x5Au8, |state, _| {
                *state = state.wrapping_mul(73).wrapping_add(41);
                Some((*state >> 7) & 1)
            })
            .collect()
    }

    #[test]
    fn hard_decide_roundtrips_every_ladder_level() {
        for level in [1u8, 2, 3, 4, 5, 6, 7, 9, 10] {
            let (mapper, _) = speed_level_components(level).unwrap();
            let bits = pseudorandom_bits(mapper.bits_per_symbol() * 64);
            let symbols = mapper.map_bits(&bits);
            assert_eq!(hard_decide(&*mapper, &symbols), bits, "level {level}");
        }
    }

    #[test]
    fn symbol_cap_matches_transceiver_derivation() {
        assert_eq!(symbols_needed(6), 324);
        assert_eq!(symbols_needed(1), 1944);
    }

    #[test]
    fn reconstructed_tx_bits_match_the_real_transmit_path() {
        let profile = CoppaProfile::vhf_wide();
        let payload = vec![0xA5; 64];
        let frame = build_diagnostic_frame(&profile, 9, &payload);
        let tx = CoppaTransceiver::new(profile, 1);
        let (_, decoded, _) = tx.receive(&frame.signal).expect("self-built frame decodes");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn raw_pre_fec_ber_is_near_zero_on_a_clean_channel_at_level_9() {
        let profile = CoppaProfile::vhf_wide();
        let modem = CoppaModem::new(profile.clone(), 1);
        let (mapper, _) = speed_level_components(9).unwrap();
        let mut errors = 0;
        let mut compared_total = 0;
        for trial in 0..8u64 {
            let seed = 0x0D1A_6005_u64.wrapping_mul(trial + 1);
            let payload: Vec<u8> = (0..64)
                .map(|i| ((seed + i).wrapping_mul(2_654_435_761) >> 24) as u8)
                .collect();
            let frame = build_diagnostic_frame(&profile, 9, &payload);
            let (_, equalized, _, _) = modem.demodulate_frame(&frame.signal).expect("demodulates");
            let decided = hard_decide(&*mapper, &equalized[..symbols_needed(6)]);
            let compared = decided.len().min(frame.interleaved_bits.len());
            errors += decided[..compared]
                .iter()
                .zip(&frame.interleaved_bits[..compared])
                .filter(|(a, b)| a != b)
                .count();
            compared_total += compared;
        }
        let raw_ber = errors as f64 / compared_total as f64;
        assert!(
            raw_ber < 0.01,
            "raw pre-FEC BER {raw_ber} on a clean channel"
        );
    }
}
