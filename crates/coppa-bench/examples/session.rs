//! `session` bench: simulated 10-minute ARQ sessions over a slowly SNR-ramping
//! Watterson channel, scoring connection survival + net goodput (Task 8,
//! decision 9(b) -- the IONOS-study metric).
//!
//! Run: `cargo run -p coppa-bench --release --example session`
//!
//! Drives the REAL `ArqTx`/`ArqRx` selective-repeat state machines
//! (`coppa_protocol::arq`) through a simulated 10-minute (600s) session per
//! trial. Simulated time is a manually-advanced `std::time::Instant`, NOT real
//! wall-clock sleeping -- following the exact pattern this codebase's own
//! `crates/coppa-protocol/src/arq.rs::test_fade_recovery_within_two_rtos`
//! already establishes for ARQ timing simulation. Every "frame" is a REAL
//! `CoppaTransceiver::transmit`/`receive` round trip through a real Watterson +
//! AWGN channel realization (not a synthetic pass/fail coin flip), so the
//! measured drop/goodput numbers reflect the actual PHY+FEC+ARQ stack.
//!
//! ## SNR ramp profile
//!
//! Linear 20 dB -> 0 dB over the first 5 simulated minutes, then 0 dB -> 20 dB
//! over the second 5 (a "fade down through the middle of the session, then
//! recover" profile -- the brief's own example schedule).
//!
//! ## Drop definition (brief, verbatim)
//!
//! "A drop = the RTO cap being reached (an unrecoverable link failure, not a
//! single retransmit)." Concretely: a segment's `ArqTx::is_failed` becomes true
//! (transmit_count exceeds `max_retransmit`) at any point during the session.
//! The session sim stops immediately at that point (an unrecoverable link
//! failure ends the session, matching how a real station would declare the
//! link down rather than keep hammering a dead channel).
//!
//! ## Acceptance target (brief, verbatim)
//!
//! "zero drops on good/moderate at any point where connect succeeded; report
//! poor honestly." Every simulated session here "connects" trivially (there is
//! no separate connect handshake being modeled, just continuous ARQ data
//! transfer from t=0), so the target reduces to: zero drops across all
//! good/moderate sessions.

use std::time::{Duration, Instant};

use coppa_bench::scenario::{mode_for_level, select_profile, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::modem::{frame_airtime_s, CoppaTransceiver};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// Speed level used for the whole session (fixed, not rate-adaptive -- rate
/// adaptation is a separate mechanism, `coppa-ml`'s `RateLoop`, exercised by
/// `closed_loop_arq.rs`; this bench isolates ARQ robustness itself). Matches
/// `arq::DEFAULT_SPEED_LEVEL`.
const LEVEL: u8 = 2;

/// ARQ window size.
const WINDOW: u8 = 8;

/// Half-duplex turnaround, matching `arq::DEFAULT_TURNAROUND`.
const TURNAROUND: Duration = Duration::from_millis(150);

/// Simulated session duration: 10 minutes.
const SESSION_DURATION: Duration = Duration::from_secs(600);

/// Sessions per Watterson preset. Kept modest (each session drives ~400-600
/// real transceiver transmit/receive round trips) so the whole bench finishes
/// in a couple of minutes in release mode; increase for a tighter statistical
/// picture if needed.
const SESSIONS_PER_PRESET: usize = 5;

/// SNR (dB) at simulated elapsed time `t` seconds into the session: linear
/// ramp 20 -> 0 over the first half, then 0 -> 20 over the second half.
fn ramp_snr_db(elapsed_s: f64, total_s: f64) -> f32 {
    let half = total_s / 2.0;
    if elapsed_s <= half {
        (20.0 - 20.0 * (elapsed_s / half)) as f32
    } else {
        let t2 = (elapsed_s - half).min(half);
        (20.0 * (t2 / half)) as f32
    }
}

fn make_header(level: u8, payload_len: u16, seq: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: level,
        seq_num: seq,
        payload_len,
        codewords: 1,
    }
}

/// Transmit `data` through the real transceiver + a Watterson(preset) + AWGN(snr_db)
/// channel realization (3 kHz-referenced convention, matching `runner::run_trial`),
/// and report whether it decoded back to exactly `data`.
fn try_send_frame(
    tx_phy: &CoppaTransceiver,
    level: u8,
    seq: u8,
    data: &[u8],
    preset: WattersonPreset,
    snr_db: f32,
    seed: u64,
) -> (bool, usize) {
    let header = make_header(level, data.len() as u16, seq);
    let clean = tx_phy
        .transmit(&header, data)
        .expect("payload sized from this level's own payload_bytes() always fits");
    let frame_samples = clean.len();
    let sr = SAMPLE_RATE as f32;

    let p_clean = coppa_channel::mean_power(&clean);
    let noise_seed = seed ^ 0x5555_5555_5555_5555;
    let faded = coppa_channel::watterson::watterson_preset(
        &clean,
        sr,
        preset,
        seed ^ 0x3333_3333_3333_3333,
    );
    let faded = coppa_channel::awgn_ref_seeded(&faded, snr_db, p_clean, sr, noise_seed);

    let ok = match tx_phy.receive(&faded) {
        Ok((_h, bytes, _lvl)) => bytes.len() >= data.len() && bytes[..data.len()] == data[..],
        Err(_) => false,
    };
    (ok, frame_samples)
}

struct SessionResult {
    dropped: bool,
    elapsed_s: f64,
    bytes_delivered: usize,
}

/// Run one simulated 10-minute ARQ session over `preset`, seeded by `seed`.
fn run_session(preset: WattersonPreset, seed: u64) -> SessionResult {
    let payload_bytes = mode_for_level(LEVEL).expect("valid level").payload_bytes();
    let profile = select_profile(LEVEL);
    let tx_phy = CoppaTransceiver::new(profile.clone(), 1);
    let frame_airtime =
        Duration::from_secs_f64(frame_airtime_s(LEVEL, &profile).expect("level 2 is valid"));

    let config = ArqConfig::new(WINDOW, 5, Duration::from_secs(5))
        .unwrap()
        .with_airtime_params(LEVEL, TURNAROUND, profile);
    let mut arq_tx = ArqTx::new(config);
    let mut arq_rx = ArqRx::new(WINDOW);

    let start = Instant::now();
    let mut now = start;
    let mut bytes_delivered = 0usize;
    let mut dropped = false;
    let mut frame_no: u64 = 0;

    let total_s = SESSION_DURATION.as_secs_f64();

    while now.duration_since(start) < SESSION_DURATION && !dropped {
        let elapsed_s = now.duration_since(start).as_secs_f64();
        let snr = ramp_snr_db(elapsed_s, total_s);

        if arq_tx.can_send() {
            let frame_seed = seed ^ frame_no ^ 0xACE_0000;
            let mut rng = StdRng::seed_from_u64(frame_seed);
            let data: Vec<u8> = (0..payload_bytes).map(|_| rng.random::<u8>()).collect();
            frame_no += 1;
            let seq = arq_tx.send(data.clone(), now).expect("window has room");

            let (ok, frame_samples) =
                try_send_frame(&tx_phy, LEVEL, seq, &data, preset, snr, frame_seed);
            now += Duration::from_secs_f64(frame_samples as f64 / SAMPLE_RATE as f64);

            if ok {
                let delivered = arq_rx.receive(seq, data);
                bytes_delivered += delivered.iter().map(|(_, d)| d.len()).sum::<usize>();
                now += TURNAROUND;
                let (ack_num, bitmap) = arq_rx.ack_info();
                now += frame_airtime; // ACK frame airtime (same level, one frame)
                now += TURNAROUND;
                arq_tx.process_ack(ack_num, bitmap, now);
            }
        } else {
            // Window full: nothing new to send, wait for an ACK or a timeout.
            now += Duration::from_millis(500);
        }

        // Check for timed-out segments regardless of whether we just sent.
        let retransmits = arq_tx.get_retransmits(now);
        for seq in retransmits {
            arq_tx
                .mark_retransmitted(seq, now)
                .expect("seq came from get_retransmits, must be in-flight");
            if arq_tx.is_failed(seq) {
                dropped = true;
                continue;
            }
            let data = arq_tx
                .get_segment_data(seq)
                .expect("not yet evicted (is_failed is false)")
                .to_vec();
            let elapsed2 = now.duration_since(start).as_secs_f64();
            let snr2 = ramp_snr_db(elapsed2, total_s);
            let retry_seed = seed ^ (seq as u64) ^ 0xBEEF_0000;
            let (ok, frame_samples) =
                try_send_frame(&tx_phy, LEVEL, seq, &data, preset, snr2, retry_seed);
            now += Duration::from_secs_f64(frame_samples as f64 / SAMPLE_RATE as f64);
            if ok {
                let delivered = arq_rx.receive(seq, data);
                bytes_delivered += delivered.iter().map(|(_, d)| d.len()).sum::<usize>();
                now += TURNAROUND;
                let (ack_num, bitmap) = arq_rx.ack_info();
                now += frame_airtime;
                now += TURNAROUND;
                arq_tx.process_ack(ack_num, bitmap, now);
            }
        }
    }

    SessionResult {
        dropped,
        elapsed_s: now.duration_since(start).as_secs_f64(),
        bytes_delivered,
    }
}

fn preset_name(preset: WattersonPreset) -> &'static str {
    match preset {
        WattersonPreset::Good => "good",
        WattersonPreset::Moderate => "moderate",
        WattersonPreset::Poor => "poor",
    }
}

fn main() {
    println!(
        "=== Session-robustness bench: {SESSIONS_PER_PRESET} x 10-min ARQ sessions/preset ==="
    );
    println!("Level {LEVEL} (BPSK 1/2), window {WINDOW}, SNR ramp 20->0->20 dB over the session.");
    println!("Drop = ArqTx::is_failed (RTO cap reached), not a single retransmit.\n");

    let mut any_drop_good_or_moderate = false;

    for &preset in &[
        WattersonPreset::Good,
        WattersonPreset::Moderate,
        WattersonPreset::Poor,
    ] {
        let mut drops = 0usize;
        let mut total_bytes_per_min = 0.0f64;
        let mut completed_bytes_per_min = Vec::new();

        for trial in 0..SESSIONS_PER_PRESET {
            let seed = 0x5E55_1000_u64 ^ ((preset as u64) << 16) ^ (trial as u64);
            let result = run_session(preset, seed);
            let bytes_per_min = result.bytes_delivered as f64 / (result.elapsed_s / 60.0);
            total_bytes_per_min += bytes_per_min;
            if result.dropped {
                drops += 1;
            } else {
                completed_bytes_per_min.push(bytes_per_min);
            }
            println!(
                "  [{}] trial {}: {} ({:.1}s, {} bytes, {:.1} bytes/min)",
                preset_name(preset),
                trial,
                if result.dropped {
                    "DROPPED"
                } else {
                    "completed"
                },
                result.elapsed_s,
                result.bytes_delivered,
                bytes_per_min,
            );
        }

        let avg_bytes_per_min = total_bytes_per_min / SESSIONS_PER_PRESET as f64;
        println!(
            "-> {preset_name}: {}/{} sessions completed without a drop, avg net goodput {:.1} bytes/min\n",
            SESSIONS_PER_PRESET - drops,
            SESSIONS_PER_PRESET,
            avg_bytes_per_min,
            preset_name = preset_name(preset),
        );

        if drops > 0 && !matches!(preset, WattersonPreset::Poor) {
            any_drop_good_or_moderate = true;
        }
    }

    println!("=== Acceptance target: zero drops on good/moderate ===");
    if any_drop_good_or_moderate {
        println!(
            "NOT MET: at least one drop occurred on good or moderate (see per-trial log above)."
        );
    } else {
        println!(
            "MET: zero drops observed on good/moderate across {SESSIONS_PER_PRESET} sessions each."
        );
    }
    println!("(Poor is reported honestly above, not scored against a hard target -- see brief.)");
}
