//! Paired physical-layer trials used by the session HARQ diagnostic.

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::mac::{Callsign, MacPdu};
use coppa_protocol::modem::speed_levels::max_multi_payload_for_level;
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use coppa_protocol::transport::{TransportPdu, TRANSPORT_HEADER_SIZE};
use rand::{rngs::StdRng, RngExt, SeedableRng};

/// A real transceiver with session-sized, session-framed random payloads.
/// Both the diagnostic and the fast regression guard use this exact channel.
pub struct SessionHarqTrial {
    /// Receive state is reset between independent arms, retained between retries.
    pub receiver: CoppaTransceiver,
    level: u8,
    codewords: u8,
    fading: bool,
}

impl SessionHarqTrial {
    /// Construct an HF trial. Fading selects Watterson Moderate; false is AWGN.
    pub fn new(level: u8, codewords: u8, fading: bool) -> Self {
        let receiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
        Self {
            receiver,
            level,
            codewords,
            fading,
        }
    }

    /// Frame a full-capacity session PDU around independent random host bytes.
    fn wire(&self, trial: u64) -> Vec<u8> {
        let len = max_multi_payload_for_level(self.level, self.codewords).unwrap()
            - MacPdu::HEADER_SIZE
            - TRANSPORT_HEADER_SIZE;
        let mut rng = StdRng::seed_from_u64(0xC0F1_6800 ^ trial);
        let host = (0..len).map(|_| rng.random::<u8>()).collect();
        MacPdu::new_data(
            Callsign::new("W1AW").unwrap(),
            Callsign::new("VK3ABC").unwrap(),
            0,
            TransportPdu::new_reliable(0, 0, 0, host).to_bytes(),
        )
        .to_bytes()
    }

    /// Encode all RVs without changing the payload or session framing.
    pub fn packets(&self, trial: u64) -> [Vec<f32>; 4] {
        std::array::from_fn(|rv| self.packet(trial, rv as u8))
    }

    /// Encode one RV, used by calibration which never needs retransmissions.
    pub fn packet(&self, trial: u64, rv: u8) -> Vec<f32> {
        let wire = self.wire(trial);
        self.receiver
            .transmit(
                &CoppaHeader {
                    version: 1,
                    phy_mode: 0,
                    frame_type: CoppaFrameType::Data,
                    bandwidth: 1,
                    fec_type: rv,
                    speed_level: self.level,
                    seq_num: 0,
                    payload_len: wire.len() as u16,
                    codewords: self.codewords,
                },
                &wire,
            )
            .unwrap()
    }

    /// Decode one physical transmission with a new deterministic channel draw.
    /// The seed is independent of RV, so paired arms see identical channel draws.
    pub fn receive(&self, clean: &[f32], snr: f32, trial: u64, attempt: u64) -> bool {
        let seed = 0xC0F1_6800_0000 ^ (trial << 4) ^ attempt;
        let power = coppa_channel::mean_power(clean);
        let faded = if self.fading {
            coppa_channel::watterson::watterson_preset(
                clean,
                48_000.0,
                coppa_channel::watterson::WattersonPreset::Moderate,
                seed ^ 0x3333_3333,
            )
        } else {
            clean.to_vec()
        };
        let mut noisy =
            coppa_channel::awgn_ref_seeded(&faded, snr, power, 48_000.0, seed ^ 0x5555_5555);
        noisy.resize(noisy.len() + 1200, 0.0);
        matches!(self.receiver.receive(&noisy), Ok((_, bytes, _)) if bytes == self.wire(trial))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_protocol::arq::rv_for_session_attempt;

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "real PHY Monte Carlo guard; run with --release"
    )]
    fn shipped_schedule_rescues_at_level_3_awgn() {
        let phy = SessionHarqTrial::new(3, 1, false);
        let mut failures = 0;
        let mut rescued = 0;
        for trial in 0..40 {
            let packets = phy.packets(trial);
            phy.receiver.harq_evict(0);
            if !phy.receive(&packets[0], 3.0, trial, 0) {
                failures += 1;
                if phy.receive(
                    &packets[rv_for_session_attempt(3, 1) as usize],
                    3.0,
                    trial,
                    1,
                ) {
                    rescued += 1;
                }
            }
        }
        assert!(failures > 0, "operating point must exercise retransmission");
        assert!(
            rescued > 0,
            "shipped RV must rescue a failed RV0 at level 3 AWGN"
        );
    }
}
