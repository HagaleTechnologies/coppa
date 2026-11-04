//! COP-126: sync-threshold sweep of the streaming receiver's sync front end.
//!
//! For levels 1 and 2 on `hf_standard` and both `SyncDetector` front ends
//! (`new` = raw, `new_prefiltered` = band-passed and decimated by 4), prints
//! the correct-detection count out of `--trials` at each SNR of a grid, then
//! each front end's interpolated 50% and 90% points and the gain between
//! them. A detection is correct when the first candidate lies within 60
//! samples of that front end's own candidate on the noise-free stream (see
//! `coppa_bench::sync_threshold`). Noise is full-band AWGN at the bench's
//! 3 kHz SNR convention, with no RX filter.
//!
//! Finally it prints a cost line: the realtime factor of `SyncDetector::push`
//! over 10 s of uniform noise in 512-sample chunks, best of 5.
//!
//! Run: `cargo run --release -p coppa-bench --example streaming_sync_threshold`
//! (`-- --trials 20` for a quicker pass). Output is recorded in
//! BENCHMARKS.md, "COP-126".
use std::time::Instant;

use clap::Parser;
use coppa_bench::sync_threshold::{correct_detections, crossing_db, SyncFrontEnd};
use coppa_codec::ofdm::CoppaProfile;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

/// BENCHMARKS.md's base seed convention.
const SEED: u64 = 0x00C0_FFEE;
const LEVELS: [u8; 2] = [1, 2];
const FRONT_ENDS: [SyncFrontEnd; 2] = [SyncFrontEnd::Raw, SyncFrontEnd::Prefiltered];

#[derive(Parser)]
struct Args {
    /// Trials per SNR point.
    #[arg(long, default_value_t = 60)]
    trials: u32,
}

/// 1.5 to 7 dB in 0.5 dB steps, then 11.5 to 14 dB in 0.25 dB steps.
fn snr_grid() -> Vec<f32> {
    let low = (0..=11).map(|i| 1.5 + 0.5 * i as f32);
    let high = (0..=10).map(|i| 11.5 + 0.25 * i as f32);
    low.chain(high).collect()
}

/// Realtime factor of `push` over 10 s of uniform noise in 512-sample
/// chunks, best of 5 runs.
fn push_realtime_factor(front: SyncFrontEnd) -> f64 {
    let profile = CoppaProfile::hf_standard();
    let sr = profile.sample_rate as usize;
    let mut rng = StdRng::seed_from_u64(SEED);
    let noise: Vec<f32> = (0..10 * sr)
        .map(|_| rng.random_range(-1.0f32..1.0))
        .collect();
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let mut detector = front.detector(profile.clone());
        let t = Instant::now();
        for chunk in noise.chunks(512) {
            std::hint::black_box(detector.push(chunk));
        }
        best = best.min(t.elapsed().as_secs_f64() / 10.0);
    }
    best
}

fn fmt_db(x: Option<f32>) -> String {
    x.map_or_else(|| "n/a".to_string(), |v| format!("{v:.2}"))
}

fn main() {
    let args = Args::parse();
    let snrs = snr_grid();
    println!(
        "COP-126 streaming sync threshold: hf_standard, full-band AWGN (3 kHz convention), \
         {} trials per point, base seed {SEED:#010X}",
        args.trials
    );
    for level in LEVELS {
        println!("\n### Level {level}\n");
        let header: Vec<String> = snrs.iter().map(|s| format!("{s}")).collect();
        println!("| SNR (dB) | {} |", header.join(" | "));
        println!("|---|{}", "---|".repeat(snrs.len()));
        let mut points = Vec::new();
        for front in FRONT_ENDS {
            let counts: Vec<u32> = snrs
                .iter()
                .map(|&snr| correct_detections(front, level, snr, args.trials, SEED))
                .collect();
            let cells: Vec<String> = counts.iter().map(u32::to_string).collect();
            println!("| {} | {} |", front.label(), cells.join(" | "));
            let rates: Vec<f32> = counts
                .iter()
                .map(|&c| c as f32 / args.trials as f32)
                .collect();
            points.push((
                front,
                crossing_db(&snrs, &rates, 0.5),
                crossing_db(&snrs, &rates, 0.9),
            ));
        }
        println!("\n| front end | 50% point (dB) | 90% point (dB) | gain at 50% (dB) | gain at 90% (dB) |");
        println!("|---|---|---|---|---|");
        let (_, raw50, raw90) = points[0];
        for (front, p50, p90) in points {
            let gain = |raw: Option<f32>, p: Option<f32>| fmt_db(raw.zip(p).map(|(r, p)| r - p));
            println!(
                "| {} | {} | {} | {} | {} |",
                front.label(),
                fmt_db(p50),
                fmt_db(p90),
                gain(raw50, p50),
                gain(raw90, p90)
            );
        }
    }
    println!("\n### Cost\n");
    for front in FRONT_ENDS {
        println!(
            "{}: SyncDetector::push realtime factor {:.5}x (10 s of noise, 512-sample chunks, best of 5)",
            front.label(),
            push_realtime_factor(front)
        );
    }
}
