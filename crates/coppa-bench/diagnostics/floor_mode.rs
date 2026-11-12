//! COP-169: floor-mode acceptance sweep over the persistent session channel.
//!
//! Runs the production batch receiver for a waveform through
//! `coppa_channel::persistent` (fresh channel per attempt, noise lead-in with a
//! random 0-50 ms offset, a noise tail, no oracle timing or header), tallies
//! every attempt exactly once with `coppa_bench::floor_sweep`, writes the raw
//! CSV and a metadata file, prints the 50% crossing, and exits nonzero when
//! the sweep misses the ticket's bar (−8 dB AWGN, −2 dB Watterson Poor, sync
//! share under half above the crossing, no wrong payloads).
//!
//! The floor waveform itself is not built yet (see `docs/LIMITATIONS.md`,
//! COP-169), so the only waveform this measures today is `ofdm:1`, the
//! `hf_narrow` level-1 frame the floor has to beat. Its sweep is expected to
//! fail the bar; that failure is the baseline.
//!
//! Run:
//! `cargo run --release -p coppa-bench --example floor_mode -- sweep --channel awgn --trials 200 --seed 378535936 --out-dir results/cop-169/baseline-awgn`
//! (`--seed` defaults to the development base `0x00C0FFEE`; final sweeps use
//! `0x16900000`).
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use coppa_bench::floor_sweep::{
    acceptance_grid, evaluate, run_sweep, snr_grid, SweepChannel, CSV_HEADER, DEV_SEED_BASE,
    FINAL_SESSION_SEED_BASE, FINAL_SWEEP_SEED_BASE, LEAD_IN_S, TAIL_S, TRIALS_PER_POINT,
};
use coppa_channel::persistent::SnrProfile;
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::transceiver::CoppaTransceiver;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum ChannelArg {
    Awgn,
    Poor,
}

#[derive(Subcommand)]
enum Command {
    /// FER sweep of one waveform over an SNR grid.
    Sweep {
        #[arg(long, value_enum, default_value_t = ChannelArg::Awgn)]
        channel: ChannelArg,
        #[arg(long, default_value_t = -12, allow_hyphen_values = true)]
        snr_min: i32,
        #[arg(long, default_value_t = 4, allow_hyphen_values = true)]
        snr_max: i32,
        #[arg(long, default_value_t = 2)]
        snr_step: i32,
        #[arg(long, default_value_t = 200)]
        trials: usize,
        #[arg(long, default_value_t = DEV_SEED_BASE)]
        seed: u64,
        #[arg(long)]
        out_dir: PathBuf,
    },
}

/// The waveform under test. Only the OFDM level-1 baseline exists today.
const WAVEFORM: &str = "ofdm:1";
const LEVEL: u8 = 1;

fn main() -> anyhow::Result<ExitCode> {
    let Args { command } = Args::parse();
    let Command::Sweep {
        channel,
        snr_min,
        snr_max,
        snr_step,
        trials,
        seed,
        out_dir,
    } = command;
    let channel = match channel {
        ChannelArg::Awgn => SweepChannel::Awgn,
        ChannelArg::Poor => SweepChannel::Poor,
    };
    let grid = snr_grid(snr_min, snr_max, snr_step);
    anyhow::ensure!(!grid.is_empty(), "empty SNR grid");
    anyhow::ensure!(trials > 0, "--trials must be positive");

    let tx = CoppaTransceiver::new(CoppaProfile::hf_narrow(), 1);
    let payload_bytes = coppa_protocol::modem::speed_levels::max_payload_for_level(LEVEL)
        .ok_or_else(|| anyhow::anyhow!("no level {LEVEL}"))?;
    let points = run_sweep(&tx, LEVEL, payload_bytes, channel, &grid, trials, seed)?;

    fs::create_dir_all(&out_dir)?;
    let mut csv = String::from(CSV_HEADER);
    csv.push('\n');
    for p in &points {
        let row = p.csv_row(WAVEFORM, "batch", channel.name());
        println!("{row}");
        csv.push_str(&row);
        csv.push('\n');
    }
    fs::write(out_dir.join("sweep.csv"), csv)?;

    let verdict = evaluate(&points, channel.bar_db());
    // Only the locked grid, trial count and final seed base make an acceptance
    // run; anything else is a development measurement.
    let acceptance_run =
        grid == acceptance_grid() && trials == TRIALS_PER_POINT && seed == FINAL_SWEEP_SEED_BASE;
    let ramp = SnrProfile::cop169_ramp();
    let mut meta = format!(
        "waveform={WAVEFORM}\nprofile=hf_narrow\nlevel={LEVEL}\npayload_bytes={payload_bytes}\n\
         rx_path=batch\nchannel={}\nsnr_grid_db={grid:?}\ntrials_per_point={trials}\n\
         seed_base={seed:#x}\nseed_rule=attempt_seed(base, point, trials, trial) = base + point*trials + trial; \
         channel streams = coppa_channel::persistent::derive_seed(attempt_seed, direction, stream)\n\
         lead_in_s={LEAD_IN_S} (+ uniform 0-0.05 s)\ntail_s={TAIL_S}\nsnr_reference=clean frame mean power, 3 kHz\n\
         crossing={:?}\nbar_db={}\nsync_share_violations={:?}\nwrong_payloads={}\npasses={}\n",
        channel.name(),
        verdict.crossing,
        verdict.bar_db,
        verdict.sync_share_violations,
        verdict.wrong_payloads,
        verdict.passes(),
    );
    meta.push_str(&format!(
        "acceptance_run={acceptance_run}\nfinal_sweep_seed_base={FINAL_SWEEP_SEED_BASE:#x}\n\
         final_session_seed_base={FINAL_SESSION_SEED_BASE:#x}\nsession_ramp={ramp:?} ({} s)\n",
        ramp.duration_s()
    ));
    fs::write(out_dir.join("metadata.txt"), &meta)?;
    print!("{meta}");
    Ok(if verdict.passes() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
