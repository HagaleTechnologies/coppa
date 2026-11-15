//! CLI for the Coppa PHY measurement harness (AWGN sweep over all modes),
//! plus the COP-170 `compare` subcommand.

use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use coppa_bench::compare::grid::{Channel, SIZES, SNRS_DB};
use coppa_bench::compare::link_stack::{row_spec, TrialOptions, ROWS};
use coppa_bench::compare::render::render_tables;
use coppa_bench::compare::runner::{csv_path, run_plan, Plan};
use coppa_bench::compare::trial::read_csv;

use coppa_bench::report::{to_csv, to_markdown};
use coppa_bench::runner::run_scenario;
use coppa_bench::scenario::{mode_for_level, ChannelSpec, ModeInfo, Scenario, MODES};

#[derive(Debug, Parser)]
#[command(
    name = "coppa-bench",
    about = "BER/FER/goodput-vs-SNR for Coppa PHY modes",
    allow_negative_numbers = true,
    args_conflicts_with_subcommands = true
)]
struct Args {
    /// Subcommand; omitted runs the flat AWGN/Watterson sweep below.
    #[command(subcommand)]
    cmd: Option<Cmd>,
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
    /// Benchmark OFDM profile: default (per-level) | standard | standard-short-cp | robust | narrow.
    #[arg(long, default_value = "default")]
    profile: String,
    /// Measure one speed level (1-9); omitted means all nine levels.
    #[arg(long, value_parser = parse_level)]
    level: Option<u8>,
    /// Carrier frequency offset (Hz) applied after the channel; 0.0 = none.
    #[arg(long, default_value_t = 0.0)]
    cfo: f32,
    /// Emulate a realistic SSB rig audio passband (300-2700 Hz) on the TX signal
    /// before the channel, in addition to the transceiver's own RX bandpass.
    #[arg(long, default_value_t = false)]
    ssb: bool,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run coppa through the WGN/MPG/MPP grid of the 2020 Winlink comparison
    /// and print one Markdown table per channel and transfer size.
    Compare(CompareArgs),
}

#[derive(Debug, clap::Args)]
struct CompareArgs {
    /// Rows to measure, comma-separated (link-l2, link-l4, link-default).
    #[arg(long, default_value = "link-l2", value_delimiter = ',', value_parser = parse_row)]
    rows: Vec<String>,
    /// Channels, comma-separated (wgn, mpg, mpp).
    #[arg(long, default_value = "wgn,mpg,mpp", value_delimiter = ',', value_parser = parse_compare_channel)]
    channel: Vec<Channel>,
    /// SNR points in dB, comma-separated (from 30,25,20,15,10,5,0).
    #[arg(long, default_value = "30,25,20,15,10,5,0", value_delimiter = ',', value_parser = parse_snr)]
    snr: Vec<i32>,
    /// Transfer sizes in bytes, comma-separated (2048, 16384).
    #[arg(long, default_value = "2048,16384", value_delimiter = ',', value_parser = parse_size)]
    size: Vec<usize>,
    /// Trials per cell.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=65_536))]
    trials: u32,
    /// Seed of trial 0 of cell 0 (decimal or 0x-prefixed hex).
    #[arg(long, default_value = "0x17000000", value_parser = parse_seed)]
    seed_base: u64,
    /// Trials run in parallel; default: available CPUs.
    #[arg(long)]
    jobs: Option<usize>,
    /// One CSV per row; recorded trials are kept and skipped.
    /// Default: <repo>/results/compare.
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// Measure only; print no tables.
    #[arg(long, default_value_t = false)]
    no_render: bool,
}

fn parse_row(s: &str) -> Result<String, String> {
    row_spec(s.trim())
        .map(|r| r.name.to_string())
        .ok_or_else(|| {
            let names: Vec<_> = ROWS.iter().map(|r| r.name).collect();
            format!("unknown row '{s}' (expected one of {})", names.join(", "))
        })
}

fn parse_compare_channel(s: &str) -> Result<Channel, String> {
    Channel::parse(s).ok_or_else(|| format!("unknown channel '{s}' (expected wgn, mpg or mpp)"))
}

fn parse_snr(s: &str) -> Result<i32, String> {
    let v: i32 = s.trim().parse().map_err(|_| format!("invalid SNR '{s}'"))?;
    if SNRS_DB.contains(&v) {
        Ok(v)
    } else {
        Err(format!("SNR {v} dB is off the grid {SNRS_DB:?}"))
    }
}

fn parse_size(s: &str) -> Result<usize, String> {
    let v: usize = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid size '{s}'"))?;
    if SIZES.contains(&v) {
        Ok(v)
    } else {
        Err(format!("size {v} is off the grid {SIZES:?}"))
    }
}

fn parse_seed(s: &str) -> Result<u64, String> {
    let t = s.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => t.parse(),
    }
    .map_err(|_| format!("invalid seed '{s}'"))
}

/// The plan a `compare` invocation selects.
fn compare_plan(args: &CompareArgs) -> Plan {
    let jobs = args.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1)
    });
    Plan {
        rows: args.rows.iter().filter_map(|r| row_spec(r)).collect(),
        channels: args.channel.clone(),
        snrs: args.snr.clone(),
        sizes: args.size.clone(),
        trials: args.trials as usize,
        seed_base: args.seed_base,
        jobs: jobs.max(1),
        out_dir: args
            .out_dir
            .clone()
            .unwrap_or_else(coppa_bench::compare::default_out_dir),
    }
}

fn run_compare(args: &CompareArgs) -> anyhow::Result<()> {
    let plan = compare_plan(args);
    if cfg!(debug_assertions) {
        eprintln!("compare: warning: debug build; trials run about 11x slower than with --release");
    }
    let mut log = std::io::stderr();
    run_plan(&plan, TrialOptions::default(), &mut log)?;
    let mut trials = Vec::new();
    for row in &plan.rows {
        let path = csv_path(&plan.out_dir, row);
        trials.extend(read_csv(&path)?);
        eprintln!("compare: wrote {}", path.display());
    }
    if !args.no_render {
        print!(
            "{}",
            render_tables(&plan.rows, &trials, env!("CARGO_PKG_VERSION"))
        );
    }
    Ok(())
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

fn parse_level(value: &str) -> Result<u8, String> {
    let level = value
        .parse::<u8>()
        .map_err(|_| format!("invalid speed level '{value}' (expected 1-9)"))?;
    mode_for_level(level)
        .map(|_| level)
        .ok_or_else(|| format!("unknown speed level {level} (expected 1-9)"))
}

fn selected_modes(level: Option<u8>) -> impl Iterator<Item = &'static ModeInfo> {
    MODES
        .iter()
        .filter(move |mode| level.is_none_or(|selected| mode.level == selected))
}

fn main() {
    let args = Args::parse();
    if let Some(Cmd::Compare(compare)) = &args.cmd {
        if let Err(e) = run_compare(compare) {
            eprintln!("compare: error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let snrs = snr_points(args.snr_min, args.snr_max, args.snr_step);
    let chan = parse_channel(&args.channel);
    let profile_override = coppa_bench::scenario::profile_by_name(&args.profile);

    let title = match args.channel.as_str() {
        "awgn" => "AWGN".to_string(),
        other => format!("Watterson ({})", other),
    };

    let mut all_points = Vec::new();
    for mode in selected_modes(args.level) {
        let scenario = Scenario {
            level: mode.level,
            channel: chan,
            snr_db_points: snrs.clone(),
            trials: args.trials,
            seed: args.seed,
            profile_override: profile_override.clone(),
            cfo_hz: args.cfo,
            sco_ppm: 0.0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use coppa_bench::compare::grid::SEED_BASE;

    #[test]
    fn level_one_selects_only_one_mode() {
        let args = Args::try_parse_from(["coppa-bench", "--level", "1"]).unwrap();
        let levels: Vec<_> = selected_modes(args.level).map(|mode| mode.level).collect();
        assert_eq!(levels, [1]);
    }

    #[test]
    fn omitted_level_selects_all_nine_modes() {
        let args = Args::try_parse_from(["coppa-bench"]).unwrap();
        let levels: Vec<_> = selected_modes(args.level).map(|mode| mode.level).collect();
        assert_eq!(levels, (1..=9).collect::<Vec<_>>());
    }

    #[test]
    fn bare_compare_selects_the_published_plan() {
        let args = Args::try_parse_from(["coppa-bench", "compare"]).unwrap();
        let Some(Cmd::Compare(compare)) = &args.cmd else {
            panic!("expected the compare subcommand");
        };
        let plan = compare_plan(compare);
        let rows: Vec<_> = plan.rows.iter().map(|r| r.name).collect();
        assert_eq!(rows, ["link-l2"]);
        assert_eq!(plan.channels, Channel::ALL);
        assert_eq!(plan.snrs, SNRS_DB);
        assert_eq!(plan.sizes, SIZES);
        assert_eq!(plan.trials, 8);
        assert_eq!(plan.seed_base, SEED_BASE);
        assert!(plan.jobs >= 1);
        assert_eq!(plan.out_dir, coppa_bench::compare::default_out_dir());
        assert!(plan.out_dir.ends_with("results/compare"));
        // The default resolves against the repository root, not the cwd.
        assert!(coppa_bench::compare::repo_root()
            .join("crates/coppa-bench/Cargo.toml")
            .exists());
    }

    #[test]
    fn compare_rejects_off_grid_points() {
        assert!(Args::try_parse_from(["coppa-bench", "compare", "--snr", "12"]).is_err());
        assert!(Args::try_parse_from(["coppa-bench", "compare", "--size", "100"]).is_err());
        assert!(Args::try_parse_from(["coppa-bench", "compare", "--rows", "vara"]).is_err());
        assert!(Args::try_parse_from(["coppa-bench", "compare", "--trials", "0"]).is_err());
        let args = Args::try_parse_from([
            "coppa-bench",
            "compare",
            "--channel",
            "mpg,mpp",
            "--seed-base",
            "0x10",
        ])
        .unwrap();
        let Some(Cmd::Compare(compare)) = &args.cmd else {
            panic!("expected the compare subcommand");
        };
        assert_eq!(compare.channel, [Channel::Mpg, Channel::Mpp]);
        assert_eq!(compare.seed_base, 16);
    }

    #[test]
    fn flat_flags_do_not_mix_with_compare() {
        let error = Args::try_parse_from(["coppa-bench", "--level", "2", "compare"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        let flat = Args::try_parse_from(["coppa-bench", "--level", "2"]).unwrap();
        assert!(flat.cmd.is_none());
    }

    #[test]
    fn invalid_level_is_rejected_by_cli() {
        let error = Args::try_parse_from(["coppa-bench", "--level", "10"]).unwrap_err();
        assert!(error.to_string().contains("unknown speed level 10"));
    }
}
