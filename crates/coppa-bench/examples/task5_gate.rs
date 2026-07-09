//! Phase 2 Task 5 bench gate: one-round turbo re-estimation.
//!
//! Paired turbo-on/turbo-off sweeps (levels 2, 5, 6; awgn + watterson-{moderate,poor};
//! same seeds for both configs so the comparison is apples-to-apples) plus a dedicated
//! CPU/frame overhead measurement at each level's pre-Task-5 ~30% FER operating point on
//! watterson-poor (same noisy signal fed to both a turbo-on and turbo-off transceiver, so
//! any wall-time delta is PURELY the retry path's own cost, not decode-difficulty noise).
//!
//! Acceptance (task brief, Step 3): >= 1.0 dB gain at FER@10% on poor with turbo on; AWGN
//! unchanged; average CPU/frame <= 2x turbo overhead specifically (i.e. vs this branch's
//! turbo-off decoder, not the older pre-Task-4 codec); firing rate per SNR point recorded.
//!
//! Writes CSV to `results/task5-gate/` and prints a markdown summary + the acceptance
//! checks to stdout.
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use coppa_bench::metrics::{aggregate, bit_errors, MeasurementPoint, TrialOutcome};
use coppa_bench::report::{fer_threshold, to_csv, to_markdown};
use coppa_bench::scenario::{mode_for_level, select_profile, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::modem::transceiver::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const LEVELS: [u8; 3] = [2, 5, 6];
const TRIALS: usize = 200;
const SEED: u64 = 0x7A5C_0000;

#[derive(Clone, Copy)]
enum Channel {
    Awgn,
    Moderate,
    Poor,
}

impl Channel {
    fn name(self, turbo: bool) -> &'static str {
        match (self, turbo) {
            (Channel::Awgn, false) => "awgn-turbo-off",
            (Channel::Awgn, true) => "awgn-turbo-on",
            (Channel::Moderate, false) => "watterson-moderate-turbo-off",
            (Channel::Moderate, true) => "watterson-moderate-turbo-on",
            (Channel::Poor, false) => "watterson-poor-turbo-off",
            (Channel::Poor, true) => "watterson-poor-turbo-on",
        }
    }
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

fn snr_points() -> Vec<f32> {
    let mut v = Vec::new();
    let mut s = -6.0f32;
    while s <= 30.0 + 1e-6 {
        v.push(s);
        s += 3.0;
    }
    v
}

/// One trial: random payload -> transmit -> channel(snr) -> receive. Returns the
/// outcome, the frame's sample count (airtime), and whether turbo actually fired
/// (`tx.turbo_attempts()` incremented).
fn run_trial(
    tx: &CoppaTransceiver,
    header: &CoppaHeader,
    payload_bytes: usize,
    snr_db: f32,
    channel: Channel,
    seed: u64,
) -> (TrialOutcome, usize, bool) {
    let mut rng = StdRng::seed_from_u64(seed);
    let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
    let clean = tx
        .transmit(header, &payload)
        .expect("payload within this level's capacity");
    let frame_samples = clean.len();
    let sr = SAMPLE_RATE as f32;

    let p_clean = coppa_channel::mean_power(&clean);
    let noise_seed = seed ^ 0x5555_5555_5555_5555;
    let noisy = match channel {
        Channel::Awgn => coppa_channel::awgn_ref_seeded(&clean, snr_db, p_clean, sr, noise_seed),
        Channel::Moderate | Channel::Poor => {
            let preset = match channel {
                Channel::Moderate => WattersonPreset::Moderate,
                Channel::Poor => WattersonPreset::Poor,
                Channel::Awgn => unreachable!(),
            };
            let faded = coppa_channel::watterson::watterson_preset(
                &clean,
                sr,
                preset,
                seed ^ 0x3333_3333_3333_3333,
            );
            coppa_channel::awgn_ref_seeded(&faded, snr_db, p_clean, sr, noise_seed)
        }
    };

    let before = tx.turbo_attempts();
    let outcome = match tx.receive(&noisy) {
        Ok((_h, rx_payload, _rec_level)) => {
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
    let fired = tx.turbo_attempts() > before;
    (outcome, frame_samples, fired)
}

fn run_sweep(level: u8, channel: Channel, turbo: bool) -> (Vec<MeasurementPoint>, Vec<f64>) {
    let mode = mode_for_level(level).expect("known level");
    let payload_bytes = mode.payload_bytes();
    let profile = select_profile(level);
    let tx = CoppaTransceiver::new(profile, 1).with_turbo(turbo);
    let header = make_header(level, payload_bytes as u16);
    let channel_name = channel.name(turbo);

    let mut points = Vec::new();
    let mut fire_rates = Vec::new();
    for (si, &snr_db) in snr_points().iter().enumerate() {
        let mut outcomes = Vec::with_capacity(TRIALS);
        let mut frame_samples = 0usize;
        let mut fired_count = 0usize;
        for trial in 0..TRIALS {
            let seed = SEED
                .wrapping_add((si as u64) << 32)
                .wrapping_add(trial as u64);
            let (outcome, fs, fired) =
                run_trial(&tx, &header, payload_bytes, snr_db, channel, seed);
            frame_samples = fs;
            if fired {
                fired_count += 1;
            }
            outcomes.push(outcome);
        }
        fire_rates.push(fired_count as f64 / TRIALS as f64);
        points.push(aggregate(
            level,
            mode.name,
            channel_name,
            snr_db,
            payload_bytes,
            frame_samples,
            &outcomes,
        ));
    }
    (points, fire_rates)
}

/// CPU/frame overhead at `snr_db` on watterson-poor: feed the SAME noisy signal to a
/// turbo-on and a turbo-off transceiver (so any wall-time delta is purely the retry
/// path's own cost, not decode-difficulty variance), averaged over `trials`.
fn turbo_overhead_us_per_frame(level: u8, snr_db: f32, trials: usize) -> (f64, f64, f64) {
    let mode = mode_for_level(level).expect("known level");
    let payload_bytes = mode.payload_bytes();
    let profile = select_profile(level);
    let tx_on = CoppaTransceiver::new(profile.clone(), 1).with_turbo(true);
    let tx_off = CoppaTransceiver::new(profile, 1).with_turbo(false);
    let header = make_header(level, payload_bytes as u16);
    let sr = SAMPLE_RATE as f32;

    let mut on_total = std::time::Duration::ZERO;
    let mut off_total = std::time::Duration::ZERO;
    let mut fired = 0usize;
    for trial in 0..trials {
        let seed = 0x7075_11FEu64.wrapping_add(trial as u64);
        let mut rng = StdRng::seed_from_u64(seed);
        let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
        let clean = tx_on
            .transmit(&header, &payload)
            .expect("payload within this level's capacity");
        let p_clean = coppa_channel::mean_power(&clean);
        let noise_seed = seed ^ 0x5555_5555_5555_5555;
        let faded = coppa_channel::watterson::watterson_preset(
            &clean,
            sr,
            WattersonPreset::Poor,
            seed ^ 0x3333_3333_3333_3333,
        );
        let noisy = coppa_channel::awgn_ref_seeded(&faded, snr_db, p_clean, sr, noise_seed);

        let before = tx_on.turbo_attempts();
        let t0 = Instant::now();
        let _ = tx_on.receive(&noisy);
        on_total += t0.elapsed();
        if tx_on.turbo_attempts() > before {
            fired += 1;
        }

        let t1 = Instant::now();
        let _ = tx_off.receive(&noisy);
        off_total += t1.elapsed();
    }
    let on_us = on_total.as_secs_f64() * 1e6 / trials as f64;
    let off_us = off_total.as_secs_f64() * 1e6 / trials as f64;
    (on_us, off_us, fired as f64 / trials as f64)
}

fn main() {
    let out_dir = PathBuf::from("results/task5-gate");
    fs::create_dir_all(&out_dir).expect("create out dir");

    let mut all_points: Vec<MeasurementPoint> = Vec::new();
    // fire_rates[(level, channel_variant_name)] -> per-SNR-point firing rate, aligned
    // with snr_points() order.
    let mut fire_rate_report = String::new();

    for level in LEVELS {
        for channel in [Channel::Awgn, Channel::Moderate, Channel::Poor] {
            for turbo in [false, true] {
                eprintln!(
                    "Measuring level {level} channel {} turbo={turbo}...",
                    channel.name(turbo)
                );
                let (points, fire_rates) = run_sweep(level, channel, turbo);
                if turbo {
                    fire_rate_report
                        .push_str(&format!("level={level} channel={}\n", channel.name(true)));
                    for (snr, rate) in snr_points().iter().zip(fire_rates.iter()) {
                        fire_rate_report.push_str(&format!("  snr={snr:.1} fire_rate={rate:.3}\n"));
                    }
                }
                all_points.extend(points);
            }
        }
    }

    let csv_path = out_dir.join("all.csv");
    fs::write(&csv_path, to_csv(&all_points)).expect("write csv");
    eprintln!("Wrote {}", csv_path.display());

    let fire_path = out_dir.join("firing_rates.txt");
    fs::write(&fire_path, &fire_rate_report).expect("write firing rates");
    println!("{fire_rate_report}");

    println!("{}", to_markdown(&all_points, "Task 5 turbo gate"));

    println!("## Acceptance checks\n");
    let mut poor_gain_ok = true;
    let mut awgn_unchanged_ok = true;
    for level in LEVELS {
        for (chan_label, off_name, on_name, is_poor) in [
            ("awgn", "awgn-turbo-off", "awgn-turbo-on", false),
            (
                "watterson-moderate",
                "watterson-moderate-turbo-off",
                "watterson-moderate-turbo-on",
                false,
            ),
            (
                "watterson-poor",
                "watterson-poor-turbo-off",
                "watterson-poor-turbo-on",
                true,
            ),
        ] {
            let off_pts: Vec<MeasurementPoint> = all_points
                .iter()
                .filter(|p| p.level == level && p.channel == off_name)
                .cloned()
                .collect();
            let on_pts: Vec<MeasurementPoint> = all_points
                .iter()
                .filter(|p| p.level == level && p.channel == on_name)
                .cloned()
                .collect();
            let t_off = fer_threshold(&off_pts, level, 0.10);
            let t_on = fer_threshold(&on_pts, level, 0.10);
            let gain_db = match (t_off, t_on) {
                (Some(off), Some(on)) => Some(off - on),
                _ => None,
            };
            println!(
                "level={level} channel={chan_label} fer10_off={t_off:?} fer10_on={t_on:?} gain_db={gain_db:?}"
            );
            if is_poor {
                let ok = matches!(gain_db, Some(g) if g >= 1.0);
                poor_gain_ok &= ok;
            } else if chan_label == "awgn" {
                // "unchanged": turbo-on threshold must not be WORSE (higher) than
                // turbo-off's by more than a small margin (0.5 dB slack for sweep
                // granularity/CI noise -- points are only 3 dB apart).
                let ok = match (t_off, t_on) {
                    (Some(off), Some(on)) => on <= off + 0.5,
                    (None, None) => true,
                    _ => false,
                };
                awgn_unchanged_ok &= ok;
            }
        }
    }
    println!("\nPOOR >=1.0dB gain at FER@10% (all levels): {poor_gain_ok}");
    println!("AWGN unchanged (all levels): {awgn_unchanged_ok}");

    println!("\n## CPU/frame overhead (turbo-specific, same noisy signal both configs)\n");
    // Per-level operating point: the turbo-off SNR point closest to 30% FER on
    // watterson-poor, from the sweep just measured.
    for level in LEVELS {
        let off_pts: Vec<&MeasurementPoint> = all_points
            .iter()
            .filter(|p| p.level == level && p.channel == "watterson-poor-turbo-off")
            .collect();
        let op_point = off_pts
            .iter()
            .min_by(|a, b| (a.fer - 0.30).abs().total_cmp(&(b.fer - 0.30).abs()))
            .map(|p| p.snr_db)
            .unwrap_or(9.0);
        let (on_us, off_us, fire_rate) = turbo_overhead_us_per_frame(level, op_point, 300);
        let ratio = on_us / off_us.max(1e-9);
        println!(
            "level={level} op_snr_db={op_point:.1} fire_rate={fire_rate:.3} off_us_per_frame={off_us:.1} on_us_per_frame={on_us:.1} ratio={ratio:.3}"
        );
    }
}
