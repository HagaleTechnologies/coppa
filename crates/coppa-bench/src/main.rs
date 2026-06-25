//! CLI for the Coppa PHY measurement harness (AWGN sweep over all modes).

use std::fs;
use std::path::PathBuf;

use clap::Parser;

use coppa_bench::report::{to_csv, to_markdown};
use coppa_bench::runner::run_scenario;
use coppa_bench::scenario::{ChannelSpec, Scenario, MODES};

#[derive(Parser)]
#[command(
    name = "coppa-bench",
    about = "BER/FER/goodput-vs-SNR for Coppa PHY modes"
)]
struct Args {
    /// Trials per SNR point.
    #[arg(long, default_value_t = 100)]
    trials: usize,
    /// Minimum SNR (dB).
    #[arg(long, default_value_t = -6.0)]
    snr_min: f32,
    /// Maximum SNR (dB).
    #[arg(long, default_value_t = 30.0)]
    snr_max: f32,
    /// SNR step (dB).
    #[arg(long, default_value_t = 3.0)]
    snr_step: f32,
    /// Output directory for raw CSV.
    #[arg(long, default_value = "results")]
    out_dir: PathBuf,
    /// Base RNG seed.
    #[arg(long, default_value_t = 0x00C0_FFEE)]
    seed: u64,
}

fn snr_points(min: f32, max: f32, step: f32) -> Vec<f32> {
    let mut v = Vec::new();
    let mut s = min;
    while s <= max + 1e-6 {
        v.push(s);
        s += step;
    }
    v
}

fn main() {
    let args = Args::parse();
    let snrs = snr_points(args.snr_min, args.snr_max, args.snr_step);

    let mut all_points = Vec::new();
    for mode in MODES {
        let scenario = Scenario {
            level: mode.level,
            channel: ChannelSpec::Awgn,
            snr_db_points: snrs.clone(),
            trials: args.trials,
            seed: args.seed,
        };
        eprintln!("Measuring level {} ({})...", mode.level, mode.name);
        all_points.extend(run_scenario(&scenario));
    }

    fs::create_dir_all(&args.out_dir).expect("create out dir");
    let csv_path = args.out_dir.join("awgn.csv");
    fs::write(&csv_path, to_csv(&all_points)).expect("write csv");
    eprintln!("Wrote {}", csv_path.display());

    println!("{}", to_markdown(&all_points, "AWGN"));
}
