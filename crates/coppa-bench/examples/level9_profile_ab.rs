//! Level-9 profile × CP A/B measurement.
//!
//! Reports FER and goodput because the profiles have materially different airtime. The
//! `vhf_wide_long_cp` arm changes only the cyclic prefix, isolating CP from carrier geometry.

use std::{fs, path::PathBuf};

use coppa_bench::{
    profile_ab::profile_arms,
    report::{fer_threshold, to_csv, to_markdown},
    runner::run_scenario,
    scenario::{ChannelSpec, Scenario},
};
use coppa_channel::watterson::WattersonPreset;

const LEVELS: [u8; 2] = [9, 7];
const DEFAULT_TRIALS: usize = 300;
const SEED: u64 = 0x00C0_0004;

fn snr_points() -> Vec<f32> {
    let spec = std::env::var("SNRS").unwrap_or_else(|_| "6,9,12,15,18,21,24,27,30,33,36".into());
    spec.split(',')
        .map(|s| s.trim().parse().expect("SNRS must contain numbers"))
        .collect()
}

fn main() {
    let trials = std::env::var("TRIALS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_TRIALS);
    let channels = [
        ("awgn", ChannelSpec::Awgn),
        (
            "watterson-good",
            ChannelSpec::Watterson(WattersonPreset::Good),
        ),
        (
            "watterson-moderate",
            ChannelSpec::Watterson(WattersonPreset::Moderate),
        ),
        (
            "watterson-poor",
            ChannelSpec::Watterson(WattersonPreset::Poor),
        ),
    ];
    let out_dir = PathBuf::from("results/level9-profile-ab");
    fs::create_dir_all(&out_dir).expect("create output directory");

    for (channel_name, channel) in channels {
        for (profile_name, profile) in profile_arms() {
            let mut points = Vec::new();
            for level in LEVELS {
                eprintln!("Measuring level {level} on {channel_name} ({profile_name})...");
                points.extend(run_scenario(&Scenario {
                    level,
                    channel,
                    snr_db_points: snr_points(),
                    trials,
                    seed: SEED,
                    profile_override: Some(profile.clone()),
                    cfo_hz: 0.0,
                    sco_ppm: 0.0,
                    ssb: false,
                }));
            }
            fs::write(
                out_dir.join(format!("{channel_name}_{profile_name}.csv")),
                to_csv(&points),
            )
            .expect("write CSV");
            println!(
                "{}",
                to_markdown(&points, &format!("{channel_name} ({profile_name})"))
            );
            for level in LEVELS {
                println!(
                    "SUMMARY level={level} channel={channel_name} profile={profile_name} fer10_threshold_db={:?} fer01_threshold_db={:?}",
                    fer_threshold(&points, level, 0.10),
                    fer_threshold(&points, level, 0.01),
                );
            }
        }
    }
}
