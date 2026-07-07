//! Phase 2 Task 7 bench gate: same paired sweep as `task1_gate.rs` (levels 2 and 5,
//! watterson-{moderate,poor} + awgn, 400 trials per SNR point, same seed), run
//! against this branch's Kalman-tracked delay-domain estimator so the numbers are
//! directly comparable to `results/p1-hotfix/{moderate,poor,awgn}.csv` (the
//! pre-Task-1 baseline) and Task 1's own (regressed) numbers.
//!
//! Prints a markdown table plus the FER@10%-CI threshold per (level, channel), and
//! writes raw CSV to `results/task7-gate/`.
use std::fs;
use std::path::PathBuf;

use coppa_bench::report::{fer_threshold, to_csv, to_markdown};
use coppa_bench::runner::run_scenario;
use coppa_bench::scenario::ChannelSpec;
use coppa_bench::scenario::Scenario;
use coppa_channel::watterson::WattersonPreset;

const LEVELS: [u8; 2] = [2, 5];
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

    let out_dir = PathBuf::from("results/task7-gate");
    fs::create_dir_all(&out_dir).expect("create out dir");

    for (name, chan) in channels {
        let mut all_points = Vec::new();
        for level in LEVELS {
            let scenario = Scenario {
                level,
                channel: chan,
                snr_db_points: snr_points(),
                trials: TRIALS,
                seed: SEED,
                profile_override: None,
                cfo_hz: 0.0,
                ssb: false,
            };
            eprintln!("Measuring level {level} on {name}...");
            all_points.extend(run_scenario(&scenario));
        }
        let csv_path = out_dir.join(format!("{name}.csv"));
        fs::write(&csv_path, to_csv(&all_points)).expect("write csv");
        eprintln!("Wrote {}", csv_path.display());

        println!("{}", to_markdown(&all_points, name));
        for level in LEVELS {
            let t10 = fer_threshold(&all_points, level, 0.10);
            let t01 = fer_threshold(&all_points, level, 0.01);
            println!(
                "SUMMARY level={level} channel={name} fer10_threshold_db={:?} fer01_threshold_db={:?}",
                t10, t01
            );
        }
    }
}
