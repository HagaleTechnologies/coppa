//! Task 5 follow-up measurement: turbo "rescue rate" -- of the frames where the
//! first-pass LDPC decode failed and turbo fired, what fraction ultimately
//! succeeded (both at the LDPC-convergence level via `turbo_rescues()`/
//! `turbo_attempts()`, and at the bench's actual payload-match success
//! criterion)?
//!
//! This is a direct, FER-threshold-independent measurement, run to disambiguate
//! `task5_gate.rs`'s finding that watterson-poor (and level 5/6's
//! watterson-moderate) never crosses the FER<=10% threshold for EITHER turbo
//! setting within -6..30 dB -- meaning the brief's "dB gain at FER@10%"
//! acceptance metric is undefined there, not necessarily that turbo has no
//! effect. Picks, per (level, channel), the SNR point from
//! `results/task5-gate/firing_rates.txt` with the highest observed turbo
//! firing rate (i.e. the point with the most statistical power for measuring
//! a rescue rate), and runs a larger trial count there.
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const TRIALS: usize = 400;

fn make_header(level: u8, payload_len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len,
    }
}

enum Channel {
    Poor,
    Moderate,
}

/// (level, channel, snr_db, payload_bytes, profile) -- snr_db chosen as the
/// highest-firing-rate point from the just-completed `task5_gate` sweep's
/// `firing_rates.txt` for that (level, channel).
fn cases() -> Vec<(u8, Channel, f32, usize, coppa_codec::ofdm::CoppaProfile)> {
    use coppa_codec::ofdm::CoppaProfile;
    vec![
        (2, Channel::Poor, 6.0, 121, CoppaProfile::hf_standard()), // fire_rate=0.385
        (2, Channel::Moderate, 12.0, 121, CoppaProfile::hf_standard()), // fire_rate=0.125
        (5, Channel::Poor, 30.0, 162, CoppaProfile::vhf_wide()),   // fire_rate=0.470
        (5, Channel::Moderate, 15.0, 162, CoppaProfile::vhf_wide()), // fire_rate=0.255
        (6, Channel::Poor, 27.0, 121, CoppaProfile::vhf_wide()),   // fire_rate=0.470
        (6, Channel::Moderate, 15.0, 121, CoppaProfile::vhf_wide()), // fire_rate=0.235
    ]
}

fn main() {
    for (level, channel, snr_db, payload_bytes, profile) in cases() {
        let tx = CoppaTransceiver::new(profile, 1); // turbo on (default)
        let header = make_header(level, payload_bytes as u16);
        let sr = 48_000.0f32;

        let mut fired = 0u64;
        let mut rescued_payload = 0u64; // fired AND final payload matched
        let mut first_pass_failures_would_be = 0u64; // fired (proxy: any first-pass failure)
        let mut overall_success = 0u64;

        for trial in 0..TRIALS {
            let seed = 0xF00D_0000u64
                .wrapping_add((level as u64) << 40)
                .wrapping_add(trial as u64);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
            let clean = tx.transmit(&header, &payload);
            let p_clean = coppa_channel::mean_power(&clean);
            let noise_seed = seed ^ 0x5555_5555_5555_5555;
            let preset = match channel {
                Channel::Poor => coppa_channel::watterson::WattersonPreset::Poor,
                Channel::Moderate => coppa_channel::watterson::WattersonPreset::Moderate,
            };
            let faded = coppa_channel::watterson::watterson_preset(
                &clean,
                sr,
                preset,
                seed ^ 0x3333_3333_3333_3333,
            );
            let noisy = coppa_channel::awgn_ref_seeded(&faded, snr_db, p_clean, sr, noise_seed);

            let before_attempts = tx.turbo_attempts();
            let ok =
                matches!(tx.receive(&noisy), Ok((_, rx)) if rx[..payload.len()] == payload[..]);
            let did_fire = tx.turbo_attempts() > before_attempts;

            if ok {
                overall_success += 1;
            }
            if did_fire {
                fired += 1;
                first_pass_failures_would_be += 1;
                if ok {
                    rescued_payload += 1;
                }
            }
        }

        let ldpc_rescues = tx.turbo_rescues();
        let rescue_rate_payload = if fired > 0 {
            rescued_payload as f64 / fired as f64
        } else {
            0.0
        };
        let overall_fer = 1.0 - (overall_success as f64 / TRIALS as f64);

        println!(
            "level={level} channel={} snr_db={snr_db:.1} trials={TRIALS} fired={fired} \
             ldpc_rescues={ldpc_rescues} payload_rescued={rescued_payload} \
             rescue_rate(payload)={rescue_rate_payload:.3} overall_fer={overall_fer:.3} \
             (first_pass_failures~{first_pass_failures_would_be})",
            match channel {
                Channel::Poor => "watterson-poor",
                Channel::Moderate => "watterson-moderate",
            }
        );
    }
}
