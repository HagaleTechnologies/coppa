//! Short-CP coherence-time lever gate bench: compares hf_standard (long CP,
//! 6.25 ms) against hf_standard_short_cp (3.0 ms) under real AWGN/Watterson
//! fading, at the two levels that actually route through an HF profile by
//! default (select_profile: levels 1-4 use hf_standard, 5+ use vhf_wide --
//! level 5 would NOT be a meaningful HF-profile comparison here). Reports
//! FER *and* goodput per profile variant, since hf_standard_short_cp costs
//! ~12% less airtime regardless of any FER effect.
//!
//! Phase 1 of the coherence-time lever investigated in
//! docs/superpowers/specs/2026-07-19-short-cp-fading-coherence-design.md,
//! following the fading-root-cause investigation (BENCHMARKS.md's "Fading
//! root-cause investigation" section) that found Watterson-Moderate/Poor's
//! real Doppler coherence time is much shorter than a frame -- this bench
//! measures whether hf_standard_short_cp's shorter frame duration helps.
//!
//! 400 trials per SNR point, -6..30 dB step 3, seed 0x00C0FFEE, levels 2/4,
//! channels awgn/watterson-moderate/watterson-poor. Writes CSV to
//! results/short-cp-fading-gate/{channel}_{profile}.csv.
use std::fs;
use std::path::PathBuf;

use coppa_bench::report::{fer_threshold, to_csv, to_markdown};
use coppa_bench::runner::run_scenario;
use coppa_bench::scenario::ChannelSpec;
use coppa_bench::scenario::Scenario;
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::CoppaProfile;

const LEVELS: [u8; 2] = [2, 4];
const TRIALS: usize = 400;
const SEED: u64 = 0x00C0_FFEE;

fn snr_points() -> Vec<f32> {
    let mut v = Vec::new();
    let mut s = -6.0f32;
    while s <= 30.0 + 1e-6 {
        v.push(s);
        s += 3.0;
    }
    v
}

fn main() {
    let channels = [
        ("awgn", ChannelSpec::Awgn),
        (
            "watterson-moderate",
            ChannelSpec::Watterson(WattersonPreset::Moderate),
        ),
        (
            "watterson-poor",
            ChannelSpec::Watterson(WattersonPreset::Poor),
        ),
    ];
    let profiles: [(&str, Option<CoppaProfile>); 2] = [
        ("hf_standard", None),
        ("short_cp", Some(CoppaProfile::hf_standard_short_cp())),
    ];

    let out_dir = PathBuf::from("results/short-cp-fading-gate");
    fs::create_dir_all(&out_dir).expect("create out dir");

    for (chan_name, chan) in channels {
        for (profile_name, profile_override) in &profiles {
            let mut all_points = Vec::new();
            for level in LEVELS {
                let scenario = Scenario {
                    level,
                    channel: chan,
                    snr_db_points: snr_points(),
                    trials: TRIALS,
                    seed: SEED,
                    profile_override: profile_override.clone(),
                    cfo_hz: 0.0,
                    ssb: false,
                };
                eprintln!("Measuring level {level} on {chan_name} ({profile_name})...");
                all_points.extend(run_scenario(&scenario));
            }
            let csv_path = out_dir.join(format!("{chan_name}_{profile_name}.csv"));
            fs::write(&csv_path, to_csv(&all_points)).expect("write csv");
            eprintln!("Wrote {}", csv_path.display());

            println!(
                "{}",
                to_markdown(&all_points, &format!("{chan_name} ({profile_name})"))
            );
            for level in LEVELS {
                let t10 = fer_threshold(&all_points, level, 0.10);
                let t01 = fer_threshold(&all_points, level, 0.01);
                println!(
                    "SUMMARY level={level} channel={chan_name} profile={profile_name} fer10_threshold_db={:?} fer01_threshold_db={:?}",
                    t10, t01
                );
            }
        }
    }
}
