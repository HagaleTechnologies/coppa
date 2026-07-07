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
    about = "BER/FER/goodput-vs-SNR for Coppa PHY modes",
    allow_negative_numbers = true
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
    /// Channel: awgn | good | moderate | poor (Watterson presets).
    #[arg(long, default_value = "awgn")]
    channel: String,
    /// OFDM profile: default (per-level) | standard | robust (dense pilots).
    #[arg(long, default_value = "default")]
    profile: String,
    /// Carrier frequency offset (Hz) applied after the channel; 0.0 = none.
    #[arg(long, default_value_t = 0.0)]
    cfo: f32,
    /// Emulate a realistic SSB rig audio passband (300-2700 Hz) on the TX signal
    /// before the channel, in addition to the transceiver's own RX bandpass.
    #[arg(long, default_value_t = false)]
    ssb: bool,
}

fn parse_channel(s: &str) -> ChannelSpec {
    use coppa_channel::watterson::WattersonPreset;
    match s {
        "awgn" => ChannelSpec::Awgn,
        "good" => ChannelSpec::Watterson(WattersonPreset::Good),
        "moderate" => ChannelSpec::Watterson(WattersonPreset::Moderate),
        "poor" => ChannelSpec::Watterson(WattersonPreset::Poor),
        other => {
            eprintln!("error: unknown --channel '{other}' (expected: awgn|good|moderate|poor)");
            std::process::exit(1);
        }
    }
}

fn snr_points(min: f32, max: f32, step: f32) -> Vec<f32> {
    assert!(step > 0.0, "snr_step must be positive (got {step})");
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
    let chan = parse_channel(&args.channel);
    let profile_override = coppa_bench::scenario::profile_by_name(&args.profile);

    let title = match args.channel.as_str() {
        "awgn" => "AWGN".to_string(),
        other => format!("Watterson ({})", other),
    };

    let mut all_points = Vec::new();
    for mode in MODES {
        let scenario = Scenario {
            level: mode.level,
            channel: chan,
            snr_db_points: snrs.clone(),
            trials: args.trials,
            seed: args.seed,
            profile_override: profile_override.clone(),
            cfo_hz: args.cfo,
            ssb: args.ssb,
        };
        eprintln!("Measuring level {} ({})...", mode.level, mode.name);
        all_points.extend(run_scenario(&scenario));
    }

    fs::create_dir_all(&args.out_dir).expect("create out dir");
    let csv_path = args.out_dir.join(format!("{}.csv", args.channel));
    fs::write(&csv_path, to_csv(&all_points)).expect("write csv");
    eprintln!("Wrote {}", csv_path.display());

    println!("{}", to_markdown(&all_points, &title));
}
