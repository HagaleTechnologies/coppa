//! Phase 2 Task 3 bench: short-payload (20-byte) known-pad LLR pinning gate.
//!
//! Level 2 (BPSK 1/2, 972 info bits) with a 20-byte (160-bit) payload has 812
//! zero-padded, PRBS-scrambled info bits per frame -- `CoppaTransceiver::receive`
//! now pins those to +/-64 before LDPC decode since they're known ahead of time
//! (effective code shortening). This measures the resulting FER-vs-SNR threshold
//! shift against `results/task3-baseline/` (the pre-Task-3 code, same seeds,
//! captured separately -- see the Task 3 report for the paired numbers).
//!
//! Also re-runs the *full*-payload (mode.payload_bytes(), 121 bytes -> only 4 pad
//! bits) sweep at the same level/channel so the report can confirm normal
//! full-frame traffic is unchanged within CI by this same code change.
use std::fs;
use std::path::PathBuf;

use coppa_bench::metrics::{aggregate, bit_errors, MeasurementPoint, TrialOutcome};
use coppa_bench::report::{fer_threshold, to_csv, to_markdown};
use coppa_bench::scenario::{mode_for_level, select_profile, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const LEVEL: u8 = 2;
const SHORT_PAYLOAD_BYTES: usize = 20;
const TRIALS: usize = 300;
const SEED: u64 = 0x00C0_FFEE;

#[derive(Clone, Copy)]
enum Channel {
    Awgn,
    WattersonPoor,
}

fn make_header(level: u8, payload_len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: 0,
        payload_len,
    }
}

fn run_trial(
    tx: &CoppaTransceiver,
    level: u8,
    payload_bytes: usize,
    snr_db: f32,
    channel: Channel,
    seed: u64,
) -> (TrialOutcome, usize) {
    let mut rng = StdRng::seed_from_u64(seed);
    let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();

    let header = make_header(level, payload_bytes as u16);
    let clean = tx.transmit(&header, &payload);
    let frame_samples = clean.len();
    let sr = SAMPLE_RATE as f32;

    let p_clean = coppa_channel::mean_power(&clean);
    let noise_seed = seed ^ 0x5555_5555_5555_5555;
    let faded = match channel {
        Channel::Awgn => coppa_channel::awgn_ref_seeded(&clean, snr_db, p_clean, sr, noise_seed),
        Channel::WattersonPoor => {
            let faded = coppa_channel::watterson::watterson_preset(
                &clean,
                sr,
                WattersonPreset::Poor,
                seed ^ 0x3333_3333_3333_3333,
            );
            coppa_channel::awgn_ref_seeded(&faded, snr_db, p_clean, sr, noise_seed)
        }
    };

    let outcome = match tx.receive(&faded) {
        Ok((_h, rx_payload)) => {
            let n = payload.len().min(rx_payload.len());
            let errs = bit_errors(&payload[..n], &rx_payload[..n]);
            let success =
                rx_payload.len() >= payload.len() && rx_payload[..payload.len()] == payload[..];
            TrialOutcome {
                success,
                bit_errors: errs,
                comparable: true,
            }
        }
        Err(_) => TrialOutcome {
            success: false,
            bit_errors: 0,
            comparable: false,
        },
    };
    (outcome, frame_samples)
}

fn sweep(
    payload_bytes: usize,
    channel: Channel,
    channel_name: &'static str,
    snr_points: &[f32],
) -> Vec<MeasurementPoint> {
    let mode = mode_for_level(LEVEL).expect("level 2 must be a valid mode");
    let profile = select_profile(LEVEL);
    let tx = CoppaTransceiver::new(profile, 1);

    let mut points = Vec::with_capacity(snr_points.len());
    for (si, &snr_db) in snr_points.iter().enumerate() {
        let mut outcomes = Vec::with_capacity(TRIALS);
        let mut frame_samples = 0usize;
        for trial in 0..TRIALS {
            let seed = SEED
                .wrapping_add((si as u64) << 32)
                .wrapping_add(trial as u64);
            let (outcome, fs) = run_trial(&tx, LEVEL, payload_bytes, snr_db, channel, seed);
            frame_samples = fs;
            outcomes.push(outcome);
        }
        points.push(aggregate(
            LEVEL,
            mode.name,
            channel_name,
            snr_db,
            payload_bytes,
            frame_samples,
            &outcomes,
        ));
    }
    points
}

fn snr_points() -> Vec<f32> {
    let mut v = Vec::new();
    let mut s = -9.0f32;
    while s <= 15.0 + 1e-6 {
        v.push(s);
        s += 1.5;
    }
    v
}

fn main() {
    let out_dir = PathBuf::from("results/task3-short-payload-gate");
    fs::create_dir_all(&out_dir).expect("create out dir");

    let mode = mode_for_level(LEVEL).unwrap();
    let full_payload_bytes = mode.payload_bytes();

    let scenarios: [(&str, usize, Channel); 4] = [
        ("short20-awgn", SHORT_PAYLOAD_BYTES, Channel::Awgn),
        (
            "short20-watterson-poor",
            SHORT_PAYLOAD_BYTES,
            Channel::WattersonPoor,
        ),
        ("full-awgn", full_payload_bytes, Channel::Awgn),
        (
            "full-watterson-poor",
            full_payload_bytes,
            Channel::WattersonPoor,
        ),
    ];

    for (name, payload_bytes, channel) in scenarios {
        eprintln!("Measuring level {LEVEL}, payload_bytes={payload_bytes}, scenario={name}...");
        let points = sweep(payload_bytes, channel, name, &snr_points());
        let csv_path = out_dir.join(format!("{name}.csv"));
        fs::write(&csv_path, to_csv(&points)).expect("write csv");
        eprintln!("Wrote {}", csv_path.display());
        println!("{}", to_markdown(&points, name));
        let t10 = fer_threshold(&points, LEVEL, 0.10);
        let t01 = fer_threshold(&points, LEVEL, 0.01);
        println!(
            "SUMMARY scenario={name} payload_bytes={payload_bytes} fer10_threshold_db={:?} fer01_threshold_db={:?}",
            t10, t01
        );
    }
}
