//! COP-169: floor-mode acceptance sweep over the persistent session channel.
//!
//! Runs the production receiver for a waveform through
//! `coppa_channel::persistent` (fresh channel per attempt, noise lead-in with a
//! random 0-50 ms offset, a noise tail, no oracle timing or header), tallies
//! every attempt exactly once with `coppa_bench::floor_sweep`, writes the raw
//! CSV and a metadata file, prints the 50% crossing, and exits nonzero when
//! the sweep misses the ticket's bar (−8 dB AWGN, −2 dB Watterson Poor, sync
//! share under half above the crossing, no wrong payloads).
//!
//! `--rx` selects the production receive path(s) the ticket's AC1 measures:
//! `batch` (whole-capture `CoppaTransceiver::receive`), `streaming`
//! (continuous `StreamingReceiver` chunks), or `both`. Each selected path is
//! swept over the same grid, seeds and channel realizations and gets its own
//! CSV `rx_path` rows and metadata verdict block.
//!
<<<<<<< HEAD
//! Select `--waveform floor` for the production 8 s floor codec or omit it
//! for the unchanged `ofdm:1` baseline. Floor batch and continuous receivers
//! consume the same capture per seed. Connect/ramp still require COP-190's
//! deferred Link/daemon session controller.
=======
//! The floor waveform itself is not built yet (see `docs/LIMITATIONS.md`,
//! COP-169), so the only waveform this measures today is `ofdm:1`, the
//! `hf_narrow` level-1 frame the floor has to beat. Its sweep is expected to
//! fail the bar; that failure is the baseline. `connect` and `ramp` are still
//! unimplemented: they need the production floor session first.
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
//!
//! Run:
//! `cargo run --release -p coppa-bench --example floor_mode -- sweep --rx both --channel awgn --trials 200 --seed 378535936 --out-dir results/cop-169/baseline-awgn`
//! (`--seed` defaults to the development base `0x00C0FFEE`; final sweeps use
//! `0x16900000`).
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use coppa_bench::floor_sweep::{
<<<<<<< HEAD
    acceptance_grid, evaluate, run_floor_sweep, run_sweep, snr_grid, RxPath, SweepChannel,
    CSV_HEADER, DEV_SEED_BASE, FINAL_SESSION_SEED_BASE, FINAL_SWEEP_SEED_BASE, FLOOR_TAIL_S,
    LEAD_IN_S, STREAM_CHUNK_SAMPLES, TAIL_S, TRIALS_PER_POINT,
=======
    acceptance_grid, evaluate, run_sweep, snr_grid, RxPath, SweepChannel, CSV_HEADER,
    DEV_SEED_BASE, FINAL_SESSION_SEED_BASE, FINAL_SWEEP_SEED_BASE, LEAD_IN_S, STREAM_CHUNK_SAMPLES,
    TAIL_S, TRIALS_PER_POINT,
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
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

#[derive(Clone, Copy, ValueEnum)]
enum RxArg {
    Batch,
    Streaming,
    Both,
}

impl RxArg {
    fn paths(self) -> &'static [RxPath] {
        match self {
            Self::Batch => &[RxPath::Batch],
            Self::Streaming => &[RxPath::Streaming],
            Self::Both => &[RxPath::Batch, RxPath::Streaming],
        }
    }
}

<<<<<<< HEAD
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum WaveformArg {
    #[value(name = "ofdm:1")]
    Ofdm,
    Floor,
}

impl WaveformArg {
    fn name(self) -> &'static str {
        match self {
            Self::Ofdm => "ofdm:1",
            Self::Floor => "floor",
        }
    }
}

fn validate_configuration(
    min: i32,
    max: i32,
    step: i32,
    trials: usize,
) -> anyhow::Result<Vec<i32>> {
    let grid = snr_grid(min, max, step);
    anyhow::ensure!(!grid.is_empty(), "empty SNR grid");
    anyhow::ensure!(trials > 0, "--trials must be positive");
    Ok(grid)
}

fn all_selected_pass(verdicts: &[bool]) -> bool {
    !verdicts.is_empty() && verdicts.iter().all(|&v| v)
}

=======
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
#[derive(Subcommand)]
enum Command {
    /// FER sweep of one waveform over an SNR grid.
    Sweep {
        #[arg(long, value_enum, default_value_t = WaveformArg::Ofdm)]
        waveform: WaveformArg,
        #[arg(long, value_enum, default_value_t = ChannelArg::Awgn)]
        channel: ChannelArg,
        /// Production receive path(s) to measure: batch, streaming or both.
        #[arg(long, value_enum, default_value_t = RxArg::Batch)]
        rx: RxArg,
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

const LEVEL: u8 = 1;

fn main() -> anyhow::Result<ExitCode> {
    let Args { command } = Args::parse();
    let Command::Sweep {
        waveform,
        channel,
        rx,
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
<<<<<<< HEAD
    let grid = validate_configuration(snr_min, snr_max, snr_step, trials)?;
    let paths = rx.paths();
    let payload_bytes;
    let results = match waveform {
        WaveformArg::Floor => {
            payload_bytes = coppa_protocol::modem::floor::MAX_PAYLOAD;
            run_floor_sweep(channel, paths, &grid, trials, seed)?
        }
        WaveformArg::Ofdm => {
            let tx = CoppaTransceiver::new(CoppaProfile::hf_narrow(), 1);
            payload_bytes = coppa_protocol::modem::speed_levels::max_payload_for_level(LEVEL)
                .ok_or_else(|| anyhow::anyhow!("no level {LEVEL}"))?;
            paths
                .iter()
                .map(|&path| {
                    run_sweep(
                        &tx,
                        LEVEL,
                        payload_bytes,
                        channel,
                        path,
                        &grid,
                        trials,
                        seed,
                    )
                    .map(|points| (path, points))
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        }
    };
    let waveform_name = waveform.name();
    let tail_s = if waveform == WaveformArg::Floor {
        FLOOR_TAIL_S
    } else {
        TAIL_S
    };
=======
    let grid = snr_grid(snr_min, snr_max, snr_step);
    anyhow::ensure!(!grid.is_empty(), "empty SNR grid");
    anyhow::ensure!(trials > 0, "--trials must be positive");
    let paths = rx.paths();

    let tx = CoppaTransceiver::new(CoppaProfile::hf_narrow(), 1);
    let payload_bytes = coppa_protocol::modem::speed_levels::max_payload_for_level(LEVEL)
        .ok_or_else(|| anyhow::anyhow!("no level {LEVEL}"))?;

>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
    fs::create_dir_all(&out_dir)?;
    let mut csv = String::from(CSV_HEADER);
    csv.push('\n');
    // Only the locked grid, trial count and final seed base make an acceptance
    // run; anything else is a development measurement.
    let acceptance_run =
        grid == acceptance_grid() && trials == TRIALS_PER_POINT && seed == FINAL_SWEEP_SEED_BASE;
    let ramp = SnrProfile::cop169_ramp();
    let mut meta = format!(
<<<<<<< HEAD
        "waveform={waveform_name}\nproduction=true\ncomplete=true\n\npayload_bytes={payload_bytes}\n\
         channel={}\nsnr_grid_db={grid:?}\ntrials_per_point={trials}\n\
         seed_base={seed:#x}\nseed_rule=attempt_seed(base, point, trials, trial) = base + point*trials + trial; \
         channel streams = coppa_channel::persistent::derive_seed(attempt_seed, direction, stream)\n\
         lead_in_s={LEAD_IN_S} (+ uniform 0-0.05 s)\ntail_s={tail_s}\nsnr_reference=clean frame mean power, 3 kHz\n\
=======
        "waveform={WAVEFORM}\nprofile=hf_narrow\nlevel={LEVEL}\npayload_bytes={payload_bytes}\n\
         channel={}\nsnr_grid_db={grid:?}\ntrials_per_point={trials}\n\
         seed_base={seed:#x}\nseed_rule=attempt_seed(base, point, trials, trial) = base + point*trials + trial; \
         channel streams = coppa_channel::persistent::derive_seed(attempt_seed, direction, stream)\n\
         lead_in_s={LEAD_IN_S} (+ uniform 0-0.05 s)\ntail_s={TAIL_S}\nsnr_reference=clean frame mean power, 3 kHz\n\
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
         rx_paths={:?}\nstream_chunk_samples={STREAM_CHUNK_SAMPLES}\n\
         acceptance_run={acceptance_run}\nfinal_sweep_seed_base={FINAL_SWEEP_SEED_BASE:#x}\n\
         final_session_seed_base={FINAL_SESSION_SEED_BASE:#x}\nsession_ramp={ramp:?} ({} s)\n",
        channel.name(),
        paths.iter().map(|p| p.name()).collect::<Vec<_>>(),
        ramp.duration_s(),
    );
    // Every measured receive path must meet the bar on its own.
<<<<<<< HEAD
    meta.push_str(&format!(
        "invocation={}\n",
        std::env::args().collect::<Vec<_>>().join(" ")
    ));
    if waveform == WaveformArg::Ofdm {
        meta.push_str(&format!("profile=hf_narrow\nlevel={LEVEL}\n"));
    }
    let mut verdicts = Vec::new();
    for (path, points) in results {
        for p in &points {
            let row = p.csv_row(waveform_name, path.name(), channel.name());
=======
    let mut all_pass = true;
    for &path in paths {
        let points = run_sweep(
            &tx,
            LEVEL,
            payload_bytes,
            channel,
            path,
            &grid,
            trials,
            seed,
        )?;
        for p in &points {
            let row = p.csv_row(WAVEFORM, path.name(), channel.name());
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
            println!("{row}");
            csv.push_str(&row);
            csv.push('\n');
        }
        let verdict = evaluate(&points, channel.bar_db());
        meta.push_str(&format!(
            "rx_path={}\ncrossing={:?}\nbar_db={}\nsync_share_violations={:?}\n\
             wrong_payloads={}\npasses={}\n",
            path.name(),
            verdict.crossing,
            verdict.bar_db,
            verdict.sync_share_violations,
            verdict.wrong_payloads,
            verdict.passes(),
        ));
<<<<<<< HEAD
        verdicts.push(verdict.passes());
    }
    let all_pass = all_selected_pass(&verdicts);
    meta.push_str(&format!("all_selected_pass={all_pass}\n"));
=======
        all_pass &= verdict.passes();
    }
>>>>>>> 456661e52470aee3cd02511b9442932d0330b409
    fs::write(out_dir.join("sweep.csv"), csv)?;
    fs::write(out_dir.join("metadata.txt"), &meta)?;
    print!("{meta}");
    Ok(if all_pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &[&str]) -> Command {
        let mut args = vec!["floor_mode", "sweep", "--out-dir", "/tmp/cop191-parser"];
        args.extend_from_slice(extra);
        Args::try_parse_from(args).unwrap().command
    }

    #[test]
    fn waveform_and_receiver_selection() {
        for (name, count) in [("batch", 1), ("streaming", 1), ("both", 2)] {
            let Command::Sweep { waveform, rx, .. } = parse(&["--waveform", "floor", "--rx", name]);
            assert_eq!(waveform, WaveformArg::Floor);
            assert_eq!(rx.paths().len(), count);
        }
    }

    #[test]
    fn baseline_defaults_are_preserved() {
        for args in [vec![], vec!["--waveform", "ofdm:1"]] {
            let Command::Sweep { waveform, rx, .. } = parse(&args);
            assert_eq!(waveform, WaveformArg::Ofdm);
            assert_eq!(rx.paths(), &[RxPath::Batch]);
        }
    }

    #[test]
    fn all_selected_verdicts_must_pass() {
        assert!(all_selected_pass(&[true, true]));
        assert!(!all_selected_pass(&[true, false]));
        assert!(!all_selected_pass(&[false, true]));
        assert!(!all_selected_pass(&[]));
    }

    #[test]
    fn invalid_configuration_is_rejected() {
        for (min, max, step, trials) in [
            (4, -12, 2, 1),
            (-12, 4, 0, 1),
            (-12, 4, -2, 1),
            (-12, 4, 2, 0),
        ] {
            assert!(validate_configuration(min, max, step, trials).is_err());
        }
    }
}
