//! Trial runner: drive `CoppaTransceiver` through a channel and collect outcomes.

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::metrics::{aggregate, bit_errors, MeasurementPoint, TrialOutcome};
use crate::scenario::{mode_for_level, select_profile, ChannelSpec, Scenario};
use coppa_channel::watterson::WattersonPreset;

fn make_header(level: u8, payload_len: u16) -> CoppaHeader {
    // phy_mode/bandwidth are fixed here to mirror CoppaCore::encode_bytes, which also
    // hardcodes them regardless of speed level; the modem selects its OFDM profile from
    // construction, not from these header fields, so the measurement stays representative.
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

/// One trial: random payload → transmit → channel(snr) → receive.
/// Returns the outcome and the transmitted-frame sample count (airtime).
fn run_trial(
    tx: &CoppaTransceiver,
    level: u8,
    payload_bytes: usize,
    snr_db: f32,
    channel: ChannelSpec,
    seed: u64,
) -> (TrialOutcome, usize) {
    let mut rng = StdRng::seed_from_u64(seed);
    let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();

    let header = make_header(level, payload_bytes as u16);
    let clean = tx.transmit(&header, &payload);
    let frame_samples = clean.len();

    let rx_samples = match channel {
        ChannelSpec::Awgn => {
            coppa_channel::awgn_seeded(&clean, snr_db, seed ^ 0x5555_5555_5555_5555)
        }
        ChannelSpec::Watterson(preset) => {
            let faded = coppa_channel::watterson::watterson_preset(
                &clean,
                crate::scenario::SAMPLE_RATE as f32,
                preset,
                seed ^ 0x3333_3333_3333_3333,
            );
            coppa_channel::awgn_seeded(&faded, snr_db, seed ^ 0x5555_5555_5555_5555)
        }
    };

    let outcome = match tx.receive(&rx_samples) {
        Ok((_h, rx_payload)) => {
            let n = payload.len().min(rx_payload.len());
            let errs = bit_errors(&payload[..n], &rx_payload[..n]);
            let success =
                rx_payload.len() >= payload.len() && rx_payload[..payload.len()] == payload[..];
            TrialOutcome {
                success,
                bit_errors: errs,
                comparable: true,
            }
        }
        Err(_) => TrialOutcome {
            success: false,
            bit_errors: 0,
            comparable: false,
        },
    };

    (outcome, frame_samples)
}

/// Run a full scenario sweep: one `MeasurementPoint` per SNR.
pub fn run_scenario(scenario: &Scenario) -> Vec<MeasurementPoint> {
    let mode = mode_for_level(scenario.level)
        .unwrap_or_else(|| panic!("unknown speed level {}", scenario.level));
    let payload_bytes = mode.payload_bytes();
    let tx = CoppaTransceiver::new(select_profile(scenario.level), 1);
    let channel_name = match scenario.channel {
        ChannelSpec::Awgn => "awgn",
        ChannelSpec::Watterson(WattersonPreset::Good) => "watterson-good",
        ChannelSpec::Watterson(WattersonPreset::Moderate) => "watterson-moderate",
        ChannelSpec::Watterson(WattersonPreset::Poor) => "watterson-poor",
    };

    let mut points = Vec::with_capacity(scenario.snr_db_points.len());
    for (si, &snr_db) in scenario.snr_db_points.iter().enumerate() {
        let mut outcomes = Vec::with_capacity(scenario.trials);
        let mut frame_samples = 0usize;
        for trial in 0..scenario.trials {
            let seed = scenario
                .seed
                .wrapping_add((si as u64) << 32)
                .wrapping_add(trial as u64);
            let (outcome, fs) = run_trial(
                &tx,
                scenario.level,
                payload_bytes,
                snr_db,
                scenario.channel,
                seed,
            );
            frame_samples = fs;
            outcomes.push(outcome);
        }
        points.push(aggregate(
            scenario.level,
            mode.name,
            channel_name,
            snr_db,
            payload_bytes,
            frame_samples,
            &outcomes,
        ));
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ChannelSpec;

    #[test]
    fn awgn_sweep_decodes_at_high_snr_and_fails_at_low_snr() {
        let scenario = Scenario {
            level: 2,
            channel: ChannelSpec::Awgn,
            snr_db_points: vec![30.0, -15.0],
            trials: 10,
            seed: 0xABCD,
        };
        let points = run_scenario(&scenario);
        assert_eq!(points.len(), 2);
        assert!(
            points[0].fer <= 0.1,
            "high SNR should decode (fer={})",
            points[0].fer
        );
        assert!(
            points[1].fer >= 0.5,
            "very low SNR should mostly fail (fer={})",
            points[1].fer
        );
        assert!(points[0].goodput_bps > 0.0);
    }
}
