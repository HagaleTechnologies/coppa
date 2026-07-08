//! Broader Watterson-fading sanity sweep for the Phase 2 CFO x level-4
//! bounded-coarse-delay fix: levels 1-4 (the HF-routed, `calibrated_bias`-using
//! levels) across Good/Moderate/Poor Watterson presets at a few representative
//! SNR points, no CFO. Meant to be run once against the unmodified baseline
//! (via `git stash`) and once against the fix, diffing the FER tables, to catch
//! any regression broader than the single `hf_standard_header_survives_
//! watterson_moderate_fading` unit test covers.
use coppa_bench::runner::run_scenario;
use coppa_bench::scenario::{ChannelSpec, Scenario};
use coppa_channel::watterson::WattersonPreset;

fn main() {
    let trials: usize = std::env::var("TRIALS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let snrs: Vec<f32> = std::env::var("SNRS")
        .unwrap_or_else(|_| "9,15,21,27".to_string())
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    let levels: Vec<u8> = std::env::var("LEVELS")
        .unwrap_or_else(|_| "1,2,3,4".to_string())
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();

    let channels = [
        ("good", ChannelSpec::Watterson(WattersonPreset::Good)),
        (
            "moderate",
            ChannelSpec::Watterson(WattersonPreset::Moderate),
        ),
        ("poor", ChannelSpec::Watterson(WattersonPreset::Poor)),
    ];

    println!("level,channel,snr_db,fer,trials");
    for &level in &levels {
        for (name, chan) in channels {
            let scenario = Scenario {
                level,
                channel: chan,
                snr_db_points: snrs.clone(),
                trials,
                seed: 0x00C0_FFEE,
                profile_override: None,
                cfo_hz: 0.0,
                ssb: false,
            };
            let points = run_scenario(&scenario);
            for p in &points {
                println!("{},{},{},{:.4},{}", level, name, p.snr_db, p.fer, trials);
            }
        }
    }
}
