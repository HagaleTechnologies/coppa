use crate::fec::ldpc::LdpcCodec;
use crate::fec::scrambler::scramble;
use crate::modem::speed_levels::speed_level_components;
use coppa_codec::ofdm::coppa_modem::{CoppaModem, SPEED_LEVELS};
use coppa_codec::ofdm::frame::CoppaHeader;
use coppa_codec::ofdm::interleaver::BlockInterleaver;
use coppa_codec::ofdm::CoppaProfile;

pub struct CoppaTransceiver {
    modem: CoppaModem,
    profile: CoppaProfile,
}

#[derive(Debug)]
pub enum ReceiveError {
    SyncFailed,
    HeaderCorrupt,
    LdpcNotConverged { iterations: usize },
    CrcMismatch,
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SyncFailed => write!(f, "preamble synchronization failed"),
            Self::HeaderCorrupt => write!(f, "header could not be parsed"),
            Self::LdpcNotConverged { iterations } => {
                write!(
                    f,
                    "LDPC decoder did not converge after {} iterations",
                    iterations
                )
            }
            Self::CrcMismatch => write!(f, "CRC mismatch on decoded payload"),
        }
    }
}

impl std::error::Error for ReceiveError {}

impl CoppaTransceiver {
    pub fn new(profile: CoppaProfile, version: u8) -> Self {
        let modem = CoppaModem::new(profile.clone(), version);
        Self { modem, profile }
    }

    pub fn transmit(&self, header: &CoppaHeader, payload: &[u8]) -> Vec<f32> {
        use coppa_codec::traits::FecCodec;

        let (mapper, code_rate) =
            speed_level_components(header.speed_level).expect("invalid speed level in header");

        // 1. LDPC encode
        let mut codec = LdpcCodec::new(code_rate);
        let info_bits = code_rate.info_bits();
        let mut payload_bits = Vec::with_capacity(info_bits);
        for &byte in payload {
            for shift in (0..8).rev() {
                payload_bits.push((byte >> shift) & 1);
            }
        }
        payload_bits.resize(info_bits, 0u8);
        // Scramble info bits to randomize zero-padding (prevents degenerate LDPC codewords)
        scramble(&mut payload_bits);
        let coded_bits = codec.encode(&payload_bits);

        // 2. Interleave
        let data_carriers = self.profile.data_carriers;
        let interleaver = BlockInterleaver::new(coded_bits.len(), data_carriers);
        let interleaved = interleaver.interleave(&coded_bits);

        // 3. Constellation map
        let symbols = mapper.map_bits(&interleaved);

        // 4. OFDM modulate
        // Look up PAPR target from speed level table
        let sl = SPEED_LEVELS
            .iter()
            .find(|s| s.level == header.speed_level)
            .expect("invalid speed level in header");
        self.modem
            .modulate_mapped(header, &symbols, sl.papr_target_db)
    }

    pub fn receive(&self, samples: &[f32]) -> Result<(CoppaHeader, Vec<u8>), ReceiveError> {
        // 1. Demodulate to soft symbols (coded symbol count derived from header speed level)
        let (header, eq_symbols, noise_vars) = self
            .modem
            .demodulate_soft_coded(samples)
            .ok_or(ReceiveError::SyncFailed)?;

        // 2. Resolve speed level components
        let (mapper, code_rate) =
            speed_level_components(header.speed_level).map_err(|_| ReceiveError::HeaderCorrupt)?;

        // 3. Soft demap: convert equalized symbols to LLRs
        let bps = mapper.bits_per_symbol();
        let coded_bits_needed: usize = 1944; // LDPC codeword length
        let symbols_needed = coded_bits_needed.div_ceil(bps);
        let mut llrs = Vec::with_capacity(coded_bits_needed);

        for (i, &sym) in eq_symbols.iter().take(symbols_needed).enumerate() {
            let nv = if i < noise_vars.len() {
                noise_vars[i].max(0.001)
            } else {
                0.01
            };
            llrs.extend(mapper.demap_soft(sym, nv));
        }
        llrs.truncate(coded_bits_needed);
        llrs.resize(coded_bits_needed, 0.0);

        // Clip LLR magnitudes to prevent numerical overflow in BP decoder
        let llr_clip = 20.0f32;
        for llr in &mut llrs {
            *llr = llr.clamp(-llr_clip, llr_clip);
        }

        // 4. De-interleave
        let data_carriers = self.profile.data_carriers;
        let interleaver = BlockInterleaver::new(coded_bits_needed, data_carriers);
        let deinterleaved = interleaver.deinterleave(&llrs);

        // 5. LDPC decode
        let codec = LdpcCodec::new(code_rate);
        let (mut decoded_bits, converged) = codec.decode_checked(&deinterleaved);
        // Descramble to undo TX-side scrambling
        scramble(&mut decoded_bits);

        if !converged {
            return Err(ReceiveError::LdpcNotConverged { iterations: 50 });
        }

        // 6. Extract payload bytes
        let payload_len = header.payload_len as usize;
        let mut payload = Vec::with_capacity(payload_len);
        for chunk in decoded_bits.chunks(8) {
            if chunk.len() == 8 && payload.len() < payload_len {
                let mut byte = 0u8;
                for (i, &bit) in chunk.iter().enumerate() {
                    byte |= (bit & 1) << (7 - i);
                }
                payload.push(byte);
            }
        }

        Ok((header, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_codec::ofdm::frame::CoppaFrameType;

    fn make_header(speed_level: u8, payload_len: u16) -> CoppaHeader {
        CoppaHeader {
            version: 1,
            phy_mode: 0,
            frame_type: CoppaFrameType::Data,
            bandwidth: 1,
            fec_type: 0,
            speed_level,
            seq_num: 0,
            payload_len,
        }
    }

    #[test]
    fn test_transceiver_bpsk_rate_half_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = b"Hello Phase C!";
        let header = make_header(2, payload.len() as u16);

        let samples = tx.transmit(&header, payload);
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, header.speed_level);
        assert_eq!(rx_header.payload_len, header.payload_len);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_qpsk_rate_half_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        let header = make_header(3, payload.len() as u16);

        let samples = tx.transmit(&header, payload.as_slice());
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, 3);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }

    #[test]
    fn test_transceiver_16qam_rate_1_2_loopback() {
        let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        let payload = vec![0x42u8; 100];
        let header = make_header(6, payload.len() as u16);

        let samples = tx.transmit(&header, payload.as_slice());
        let (rx_header, rx_payload) = tx.receive(&samples).expect("decode should succeed");

        assert_eq!(rx_header.speed_level, 6);
        assert_eq!(&rx_payload[..payload.len()], payload.as_slice());
    }
}
