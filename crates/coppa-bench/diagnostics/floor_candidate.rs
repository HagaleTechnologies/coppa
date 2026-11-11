//! COP-169 development sweep of the experimental 16-FSK candidate.
//! This does not select a winner or exercise a production session.
use clap::{Parser, ValueEnum};
use coppa_bench::floor_candidate::{Candidate, CandidateError, FRAME_SAMPLES, SAMPLE_RATE};
use coppa_bench::floor_sweep::{
    acceptance_grid, attempt_seed, evaluate, AttemptOutcome, PointTally, SweepChannel, SyncCause,
    CSV_HEADER, DEV_SEED_BASE,
};
use coppa_channel::persistent::{Direction, SnrProfile, VirtualCable};
use coppa_channel::watterson::WattersonPreset;
use rand::{rngs::StdRng, RngExt, SeedableRng};
use std::{fs, path::PathBuf, process::ExitCode, time::Instant};

#[derive(Clone, Copy, ValueEnum)]
enum Channel {
    Awgn,
    Poor,
}
#[derive(Parser)]
struct Args {
    #[arg(long, value_enum, default_value_t=Channel::Awgn)]
    channel: Channel,
    #[arg(long, default_value_t = 20)]
    trials: usize,
    #[arg(long, default_value_t=DEV_SEED_BASE)]
    seed: u64,
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    cfo: f32,
    #[arg(long)]
    ssb: bool,
    #[arg(long)]
    out_dir: PathBuf,
}
fn main() -> anyhow::Result<ExitCode> {
    let args = Args::parse();
    anyhow::ensure!(
        args.trials > 0 && args.trials <= 200,
        "trials must be 1..200"
    );
    anyhow::ensure!(
        args.cfo.is_finite() && args.cfo.abs() <= 150.0,
        "CFO must be finite and within +/-150 Hz"
    );
    let channel = match args.channel {
        Channel::Awgn => SweepChannel::Awgn,
        Channel::Poor => SweepChannel::Poor,
    };
    let fading = match args.channel {
        Channel::Awgn => None,
        Channel::Poor => Some(WattersonPreset::Poor.config()),
    };
    let candidate = Candidate::new();
    let mut points = Vec::new();
    let started = Instant::now();
    let mut rx_seconds = 0.0;
    for (pi, snr) in acceptance_grid().iter().enumerate() {
        let mut point = PointTally::new(f64::from(*snr));
        for trial in 0..args.trials {
            let seed = attempt_seed(args.seed, pi, args.trials, trial);
            let mut rng = StdRng::seed_from_u64(seed);
            let payload: Vec<u8> = (0..40).map(|_| rng.random()).collect();
            let clean = candidate
                .encode(&payload)
                .map_err(|e| anyhow::anyhow!("encode: {e:?}"))?;
            let reference = coppa_channel::mean_power(&clean);
            let mut impaired = if args.cfo == 0.0 {
                clean
            } else {
                coppa_channel::frequency_shift(&clean, args.cfo, SAMPLE_RATE as f32)
            };
            if args.ssb {
                impaired = coppa_channel::ssb_filter(&impaired, SAMPLE_RATE as f32);
            }
            let mut cable = VirtualCable::new(
                SAMPLE_RATE as f32,
                fading.as_ref(),
                Some(SnrProfile::constant(*snr as f32)),
                reference,
                seed,
            );
            let mut received = cable.idle(0.25 + rng.random::<f64>() * 0.05).at_b;
            received.extend(cable.transmit(Direction::AToB, &impaired).at_b);
            received.extend(cable.idle(0.1).at_b);
            let rx_start = Instant::now();
            let result = candidate.receive(&received);
            rx_seconds += rx_start.elapsed().as_secs_f64();
            let outcome = match result {
                Ok(got) if got == payload => AttemptOutcome::Delivered,
                Ok(_) => AttemptOutcome::WrongPayload,
                Err(CandidateError::Acquisition) => {
                    AttemptOutcome::SyncFailed(SyncCause::NoDetection)
                }
                Err(CandidateError::Header) => AttemptOutcome::SyncFailed(SyncCause::HeaderDecode),
                Err(CandidateError::Payload) => AttemptOutcome::PayloadFailed,
                Err(CandidateError::Capacity) => {
                    anyhow::bail!("unexpected receiver capacity error")
                }
            };
            point.record(outcome, FRAME_SAMPLES, payload.len());
        }
        eprintln!(
            "{} {} dB: {}",
            channel.name(),
            snr,
            point.csv_row("experimental-mfsk16", "batch", channel.name())
        );
        points.push(point);
    }
    let verdict = evaluate(&points, channel.bar_db());
    fs::create_dir_all(&args.out_dir)?;
    let mut csv = format!("{CSV_HEADER}\n");
    for point in &points {
        csv.push_str(&point.csv_row("experimental-mfsk16", "batch", channel.name()));
        csv.push('\n');
    }
    fs::write(args.out_dir.join("sweep.csv"), csv)?;
    fs::write(args.out_dir.join("metadata.txt"), format!(
        "waveform=experimental-mfsk16\nproduction=false\nacceptance_run=false\nseed_base={}\nseed_derivation=attempt_seed(base,point,trials,trial); persistent derive_seed for direction/noise/fading\ntrials_per_point={}\nchannel={}\ncfo_hz={}\nssb={}\nsample_rate={}\nsymbol_samples=960\ntones=600:50:1350\nsync_symbols=16\nheader_repetitions=4\nheader_bytes=version2,length,crc16-ibm-sdlc-be\npayload=40-byte-zero-padded-pdu,crc32-iso-hdlc-be\nfec=NR-BG2-k352-e1408-rv0-unscrambled\ninterleaver=coded[(wire_bit*31)%1408]\nframe_samples={}\nframe_airtime_s=8\nrx=bounded-batch-zero-cfo-search\nrx_wall_seconds={rx_seconds}\nwall_seconds={}\nverdict={verdict:?}\npasses={}\nclean_room=original implementation using repository primitives; no external source copied\n",
        args.seed, args.trials, channel.name(), args.cfo, args.ssb, SAMPLE_RATE, FRAME_SAMPLES,
        started.elapsed().as_secs_f64(), verdict.passes()))?;
    Ok(if verdict.passes() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
