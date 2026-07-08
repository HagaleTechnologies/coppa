//! Precise pass-rate measurement replicating
//! `coppa-protocol`'s `hf_standard_header_survives_watterson_moderate_fading`
//! regression test exactly (same channel functions/seeds/level/SNR), but with a
//! much larger trial count so we get a real percentage instead of a pass/fail
//! bool against an 80% bar. Used to verify the Phase 2 CFO x level-4 bounded
//! coarse-delay fix does not quietly erode Watterson-fading header survival
//! even though it still clears 80%.
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::transceiver::CoppaTransceiver;

fn main() {
    let trials: u64 = std::env::var("TRIALS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);

    let tx = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
    let payload = vec![0x5Au8; 20];
    let header = CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: 1,
        seq_num: 0,
        payload_len: payload.len() as u16,
    };
    let clean = tx.transmit(&header, &payload);

    let mut ok = 0u64;
    for trial in 0..trials {
        let seed = 0xFADE_0000u64.wrapping_add(trial);
        let faded = coppa_channel::watterson::watterson(
            &clean,
            48_000.0,
            &WattersonPreset::Moderate.config(),
            seed,
        );
        let noisy = coppa_channel::awgn_seeded(&faded, 21.0, seed ^ 0x55AA);
        if matches!(tx.receive(&noisy), Ok((_, rx)) if rx[..payload.len()] == payload[..]) {
            ok += 1;
        }
    }
    println!(
        "ok={ok}/{trials} ({:.2}%)",
        100.0 * ok as f64 / trials as f64
    );
}
