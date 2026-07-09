//! Trial runner: drive `CoppaTransceiver` through a channel and collect outcomes.

use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

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
        codewords: 1,
    }
}

/// One trial: random payload → transmit → channel(snr) → receive.
/// Returns the outcome and the transmitted-frame sample count (airtime).
#[allow(clippy::too_many_arguments)]
fn run_trial(
    tx: &CoppaTransceiver,
    level: u8,
    payload_bytes: usize,
    snr_db: f32,
    channel: ChannelSpec,
    cfo_hz: f32,
    ssb: bool,
    seed: u64,
) -> (TrialOutcome, usize) {
    let mut rng = StdRng::seed_from_u64(seed);
    let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();

    let header = make_header(level, payload_bytes as u16);
    let clean = tx
        .transmit(&header, &payload)
        .expect("payload within this level's capacity");
    let frame_samples = clean.len();
    let sr = crate::scenario::SAMPLE_RATE as f32;

    // Rig TX filter -> channel: emulate a realistic SSB audio passband on the
    // clean signal BEFORE Watterson/AWGN. A second RX-side filter is
    // unnecessary here — `CoppaTransceiver::receive` already applies its own
    // 250-2850 Hz RX bandpass on HF profiles.
    let tx_signal = if ssb {
        coppa_channel::ssb_filter(&clean, sr)
    } else {
        clean
    };

    // SNR convention: 3 kHz noise bandwidth, referenced to the CLEAN (post-rig-filter,
    // pre-fade) signal's power — a fade costs receive power instead of being
    // renormalized away.
    let p_clean = coppa_channel::mean_power(&tx_signal);
    let noise_seed = seed ^ 0x5555_5555_5555_5555;
    let faded = match channel {
        ChannelSpec::Awgn => {
            coppa_channel::awgn_ref_seeded(&tx_signal, snr_db, p_clean, sr, noise_seed)
        }
        ChannelSpec::Watterson(preset) => {
            let faded = coppa_channel::watterson::watterson_preset(
                &tx_signal,
                sr,
                preset,
                seed ^ 0x3333_3333_3333_3333,
            );
            coppa_channel::awgn_ref_seeded(&faded, snr_db, p_clean, sr, noise_seed)
        }
    };

    let rx_signal = if cfo_hz != 0.0 {
        coppa_channel::frequency_shift(&faded, cfo_hz, crate::scenario::SAMPLE_RATE as f32)
    } else {
        faded
    };

    let outcome = match tx.receive(&rx_signal) {
        Ok((_h, rx_payload, _rec_level)) => {
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
    let profile = scenario
        .profile_override
        .clone()
        .unwrap_or_else(|| select_profile(scenario.level));
    let tx = CoppaTransceiver::new(profile, 1);
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
                scenario.cfo_hz,
                scenario.ssb,
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

    /// Regression test: `select_profile` routes speed level >= 5 to `vhf_wide`. A
    /// prior bug left VHF profiles on an unconditioned TX path whose preamble sat
    /// ~30-34 dB hotter than the header/payload body; since this sweep's SNR is
    /// referenced to the whole frame's mean power, that imbalance silently starved
    /// the payload of its noise budget and caused 100% frame errors at every SNR,
    /// including 30 dB. Level 2 (HF) already has `awgn_sweep_decodes_at_high_snr_and_fails_at_low_snr`
    /// below; this covers the VHF-routed side of `select_profile`.
    #[test]
    fn vhf_routed_level_awgn_sweep_decodes_at_high_snr() {
        let scenario = Scenario {
            level: 5,
            channel: ChannelSpec::Awgn,
            snr_db_points: vec![30.0],
            trials: 20,
            seed: 0xABCD,
            profile_override: None,
            cfo_hz: 0.0,
            ssb: false,
        };
        let points = run_scenario(&scenario);
        assert_eq!(points.len(), 1);
        assert!(
            points[0].fer <= 0.1,
            "level 5 (VHF-routed) should decode cleanly at 30 dB AWGN (fer={})",
            points[0].fer
        );
    }

    #[test]
    fn awgn_sweep_decodes_at_high_snr_and_fails_at_low_snr() {
        let scenario = Scenario {
            level: 2,
            channel: ChannelSpec::Awgn,
            snr_db_points: vec![30.0, -15.0],
            trials: 10,
            seed: 0xABCD,
            profile_override: None,
            cfo_hz: 0.0,
            ssb: false,
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

    #[test]
    fn profile_override_is_used() {
        // hf_robust must also decode cleanly at high SNR when forced via the override.
        let scenario = Scenario {
            level: 2,
            channel: ChannelSpec::Awgn,
            snr_db_points: vec![30.0],
            trials: 5,
            seed: 0xBEEF,
            profile_override: Some(coppa_codec::ofdm::CoppaProfile::hf_robust()),
            cfo_hz: 0.0,
            ssb: false,
        };
        let points = run_scenario(&scenario);
        assert_eq!(points.len(), 1);
        assert!(
            points[0].fer < 0.2,
            "hf_robust should decode at 30 dB AWGN (fer={})",
            points[0].fer
        );
    }

    #[test]
    fn snr_is_referenced_to_clean_power_not_faded_power() {
        // With clean-reference SNR, a Watterson channel at a fixed nominal SNR must
        // show HIGHER FER than AWGN at the same nominal SNR near threshold — fades
        // now cost signal power. Level 2 at 15 dB(3kHz): AWGN decodes essentially
        // always; Watterson Poor must lose a nontrivial fraction to fades.
        //
        // ADJUSTED from the brief's 6 dB: with the new 3 kHz-referenced convention
        // (~+9 dB more noise at the same nominal dB vs the old full-Nyquist-band
        // convention), 6 dB now sits far below the level-2 threshold — AWGN itself
        // is already at fer=1, leaving no room to show fading's extra cost. Measured
        // (40 trials, seed 0x5EED): 6 dB -> awgn fer=1, poor fer=0.975 (no
        // discrimination, both at ceiling). 15 dB keeps AWGN clean (fer=0) while
        // Watterson Poor still loses a large fraction to fades (fer=0.45), which is
        // the regime this test is meant to exercise.
        let mk = |channel| Scenario {
            level: 2,
            channel,
            snr_db_points: vec![15.0],
            trials: 40,
            seed: 0x5EED,
            profile_override: None,
            cfo_hz: 0.0,
            ssb: false,
        };
        let awgn = run_scenario(&mk(ChannelSpec::Awgn));
        let poor = run_scenario(&mk(ChannelSpec::Watterson(
            coppa_channel::watterson::WattersonPreset::Poor,
        )));
        assert!(
            poor[0].fer > awgn[0].fer + 0.05,
            "fading must cost SNR: poor fer={} awgn fer={}",
            poor[0].fer,
            awgn[0].fer
        );
    }

    #[test]
    fn ssb_channel_flag_decodes_cleanly_at_high_snr() {
        // Sanity check for the `ssb` wrapper: an HF-routed level through a realistic
        // rig audio passband (300-2700 Hz) plus the transceiver's own RX bandpass
        // (250-2850 Hz) must still decode cleanly at high AWGN SNR — both filters'
        // passbands comfortably contain the HF profile's active band (350-2700 Hz).
        let scenario = Scenario {
            level: 2,
            channel: ChannelSpec::Awgn,
            snr_db_points: vec![30.0],
            trials: 20,
            seed: 0x5CAB,
            profile_override: None,
            cfo_hz: 0.0,
            ssb: true,
        };
        let points = run_scenario(&scenario);
        assert_eq!(points.len(), 1);
        assert!(
            points[0].fer <= 0.1,
            "ssb-filtered level 2 should decode cleanly at 30 dB AWGN (fer={})",
            points[0].fer
        );
    }
}
