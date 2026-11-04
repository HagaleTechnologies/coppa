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
//! ## Acceptance policy
//!
//! Good is scored against the deterministic COP-5 regression floor: at least
//! 2 of 5 sessions must complete without a drop. This floor protects the
//! reproducible baseline; it is not a production reliability guarantee.
//! Moderate and Poor remain diagnostic-only because their known physical-layer
//! and channel-estimation limitations are tracked separately. See COP-5's
//! rationale in `BENCHMARKS.md` and ADR-008.

use std::time::{Duration, Instant};

use coppa_bench::scenario::{mode_for_level, select_profile, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::mac::{Callsign, MacFrameType, MacPdu};
use coppa_protocol::modem::{frame_airtime_s, CoppaTransceiver};
use coppa_protocol::outer_code::{
    encode_block, harq_key, max_segment_len, shard_len_for_level, InsertOutcome, OuterBlockRx,
    OuterPacket,
};
use coppa_protocol::transport::{TransportPdu, TRANSPORT_HEADER_SIZE};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

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

/// Deterministic COP-5 regression baseline, not a reliability claim.
const MIN_GOOD_COMPLETIONS: usize = 2;

fn good_sessions_meet_acceptance(completed: usize) -> bool {
    completed >= MIN_GOOD_COMPLETIONS
}

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
    let mut arq_tx = ArqTx::new(config).unwrap();
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

    let mut good_completions = 0usize;

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

        if matches!(preset, WattersonPreset::Good) {
            good_completions = SESSIONS_PER_PRESET - drops;
        }
    }

    println!("=== Acceptance policy: Good deterministic regression floor ===");
    if good_sessions_meet_acceptance(good_completions) {
        println!(
            "MET: Good completed {good_completions}/{SESSIONS_PER_PRESET} sessions without a drop (required at least {MIN_GOOD_COMPLETIONS}/{SESSIONS_PER_PRESET})."
        );
    } else {
        println!(
            "NOT MET: Good completed {good_completions}/{SESSIONS_PER_PRESET} sessions without a drop (required at least {MIN_GOOD_COMPLETIONS}/{SESSIONS_PER_PRESET})."
        );
    }
    println!("Moderate and Poor are diagnostic-only; their results remain reported above.");
    run_fade_protection_arms();
}

fn rs_sessions_meet_acceptance(completed: usize) -> bool {
    completed == SESSIONS_PER_PRESET
}

fn session_mac(kind: MacFrameType, payload: Vec<u8>) -> Vec<u8> {
    let (dest, src) = if kind == MacFrameType::Ack {
        ("VK3ABC", "W1AW")
    } else {
        ("W1AW", "VK3ABC")
    };
    MacPdu::new(
        kind,
        Callsign::new(dest).unwrap(),
        Callsign::new(src).unwrap(),
        0,
        payload,
    )
    .to_bytes()
}

/// One independent physical transmission. A new seed is used on every call,
/// including retries and ACK copies; the original baseline above is unchanged.
struct SessionChannel {
    preset: Option<WattersonPreset>,
    seed: u64,
    transmission: u64,
}

impl SessionChannel {
    fn send(&mut self, phy: &CoppaTransceiver, data: &[u8], key: u8, snr: f32) -> Option<Vec<u8>> {
        let clean = phy
            .transmit(&make_header(LEVEL, data.len() as u16, key), data)
            .unwrap();
        let seed = self.seed ^ self.transmission.wrapping_mul(0x9E37_79B9);
        self.transmission += 1;
        let mut samples = if let Some(preset) = self.preset {
            let power = coppa_channel::mean_power(&clean);
            let faded = coppa_channel::watterson::watterson_preset(
                &clean,
                SAMPLE_RATE as f32,
                preset,
                seed ^ 0x3333_3333_3333_3333,
            );
            coppa_channel::awgn_ref_seeded(
                &faded,
                snr,
                power,
                SAMPLE_RATE as f32,
                seed ^ 0x5555_5555_5555_5555,
            )
        } else {
            clean
        };
        samples.resize(samples.len() + 1200, 0.0);
        phy.receive(&samples).ok().map(|(_, bytes, _)| bytes)
    }
}

/// Daemon-matched framing and window, using the shipped packetizer and ARQ.
/// This isolates the bench's independent-per-packet fading convention, rather
/// than claiming to model a continuous channel or the daemon's event loop.
fn run_framed_session(
    preset: Option<WattersonPreset>,
    seed: u64,
    outer: bool,
    ack_loss: bool,
) -> SessionResult {
    let profile = select_profile(LEVEL);
    let receiver = CoppaTransceiver::new(profile.clone(), 1);
    let ack_receiver = CoppaTransceiver::new(profile.clone(), 1);
    let airtime = Duration::from_secs_f64(frame_airtime_s(LEVEL, &profile).unwrap());
    let config = ArqConfig::new(1, 5, Duration::from_secs(5))
        .unwrap()
        .with_airtime_params(LEVEL, TURNAROUND, profile);
    let mut tx = ArqTx::new(config).unwrap();
    let mut rx = ArqRx::new(1);
    let mut block = OuterBlockRx::default();
    let mut channel = SessionChannel {
        preset,
        seed,
        transmission: 0,
    };
    let shard_len = shard_len_for_level(LEVEL).unwrap();
    let host_len = if outer {
        max_segment_len(LEVEL).unwrap()
    } else {
        mode_for_level(LEVEL).unwrap().payload_bytes() - MacPdu::HEADER_SIZE
    } - TRANSPORT_HEADER_SIZE;
    let copies = if outer { 2 } else { 1 };
    let packet_count = if outer { 12 } else { 1 };
    tx.set_segment_floor(airtime * (packet_count + copies) + TURNAROUND * 2);
    let start = Instant::now();
    let mut now = start;
    let mut bytes_delivered = 0;
    let mut dropped = false;
    let mut block_no = 0u64;
    while now.duration_since(start) < SESSION_DURATION {
        let seq = if tx.can_send() {
            let mut rng = StdRng::seed_from_u64(seed ^ block_no ^ 0xACE_0000);
            block_no += 1;
            let payload = (0..host_len).map(|_| rng.random::<u8>()).collect();
            tx.send(payload, now).unwrap()
        } else {
            let retries = tx.get_retransmits(now);
            let Some(&seq) = retries.first() else {
                now += Duration::from_millis(500);
                continue;
            };
            tx.mark_retransmitted(seq, now).unwrap();
            if tx.is_failed(seq) {
                dropped = true;
                break;
            }
            seq
        };
        let host = tx.get_segment_data(seq).unwrap().to_vec();
        let segment = TransportPdu::new_reliable(0, seq, 0, host).to_bytes();
        let packets: Vec<(u8, Vec<u8>)> = if outer {
            encode_block(seq, &segment, shard_len)
                .unwrap()
                .into_iter()
                .map(|packet| {
                    (
                        harq_key(seq, packet.index),
                        session_mac(MacFrameType::OuterData, packet.to_bytes().unwrap()),
                    )
                })
                .collect()
        } else {
            vec![(0, session_mac(MacFrameType::Data, segment))]
        };
        let mut ack_due = false;
        for (key, wire) in packets {
            let snr = ramp_snr_db(
                now.duration_since(start).as_secs_f64(),
                SESSION_DURATION.as_secs_f64(),
            );
            let decoded = channel.send(&receiver, &wire, key, snr);
            now += airtime;
            if let Some(decoded) = decoded {
                let mac = MacPdu::from_bytes(&decoded).unwrap();
                let rebuilt = if outer {
                    let packet = OuterPacket::from_bytes(&mac.payload).unwrap();
                    if packet.seq == rx.recv_base().wrapping_sub(1) {
                        ack_due = true;
                        None
                    } else if let InsertOutcome::Rebuilt(bytes) = block.insert(&packet) {
                        Some(bytes)
                    } else {
                        None
                    }
                } else {
                    Some(mac.payload)
                };
                if let Some(bytes) = rebuilt {
                    let transport = TransportPdu::from_bytes(&bytes).unwrap();
                    assert_eq!(transport.seq_num, seq);
                    for (_, host) in rx.receive(seq, transport.payload) {
                        bytes_delivered += host.len();
                    }
                    ack_due = true;
                    if outer {
                        for previous in [seq, seq.wrapping_sub(1)] {
                            for i in 0..12 {
                                receiver.harq_evict(harq_key(previous, i));
                            }
                        }
                    }
                }
            }
        }
        if ack_due {
            now += TURNAROUND;
            let (ack, bitmap) = rx.ack_info();
            let wire = session_mac(
                MacFrameType::Ack,
                TransportPdu::new_ack(0, ack, bitmap).to_bytes(),
            );
            let mut ack_received = false;
            let mut first_ack_copy = None;
            for copy in 0..copies {
                if ack_loss {
                    let snr = ramp_snr_db(
                        now.duration_since(start).as_secs_f64(),
                        SESSION_DURATION.as_secs_f64(),
                    );
                    if let Some(bytes) = channel.send(&ack_receiver, &wire, 0, snr) {
                        let mac = MacPdu::from_bytes(&bytes).unwrap();
                        let decoded = TransportPdu::from_bytes(&mac.payload).unwrap();
                        let valid = decoded.ack_num == ack && decoded.ack_bitmap == bitmap;
                        if valid && first_ack_copy.is_none() {
                            first_ack_copy = Some(copy);
                        }
                        ack_received |= valid;
                    }
                } else {
                    ack_received = true;
                }
                now += airtime;
            }
            // After its first decoded ACK, the daemon holds one more frame.
            // If only the second copy arrived, that hold extends past the pair.
            if outer && first_ack_copy == Some(1) {
                now += airtime;
            }
            now += TURNAROUND;
            if ack_received {
                tx.process_ack(ack, bitmap, now);
            }
        }
    }
    SessionResult {
        dropped,
        elapsed_s: now.duration_since(start).as_secs_f64(),
        bytes_delivered,
    }
}

fn run_fade_protection_arms() {
    println!("\n=== COP-119 daemon-matched window-1 session arms ===");
    let mut moderate_completed = 0;
    for outer in [false, true] {
        for ack_loss in [false, true] {
            let arm = if outer { "rs12-6" } else { "stop-and-wait" };
            let ack = if ack_loss { "ACK-loss" } else { "ideal-ACK" };
            let clean = run_framed_session(None, 0x5E55_1000, outer, ack_loss);
            println!(
                "-> {arm} {ack} clean: {:.1} bytes/min",
                clean.bytes_delivered as f64 * 60.0 / clean.elapsed_s
            );
            for preset in [
                WattersonPreset::Good,
                WattersonPreset::Moderate,
                WattersonPreset::Poor,
            ] {
                let results = std::thread::scope(|scope| {
                    let handles: Vec<_> = (0..SESSIONS_PER_PRESET)
                        .map(|trial| {
                            scope.spawn(move || {
                                run_framed_session(
                                    Some(preset),
                                    0x5E55_1000 ^ ((preset as u64) << 16) ^ trial as u64,
                                    outer,
                                    ack_loss,
                                )
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|handle| handle.join().unwrap())
                        .collect::<Vec<_>>()
                });
                let mut completed = 0;
                let mut goodput = 0.0;
                for (trial, result) in results.iter().enumerate() {
                    let rate = result.bytes_delivered as f64 * 60.0 / result.elapsed_s;
                    completed += usize::from(!result.dropped);
                    goodput += rate;
                    println!("  {arm} {ack} [{}] trial {trial}: {} ({:.1}s, {} bytes, {rate:.1} bytes/min)", preset_name(preset), if result.dropped { "DROPPED" } else { "completed" }, result.elapsed_s, result.bytes_delivered);
                }
                println!(
                    "-> {arm} {ack} {}: {completed}/5 completed, {:.1} bytes/min",
                    preset_name(preset),
                    goodput / SESSIONS_PER_PRESET as f64
                );
                if outer && !ack_loss && matches!(preset, WattersonPreset::Moderate) {
                    moderate_completed = completed;
                }
            }
        }
    }
    println!("{}: rs12-6 Moderate completed {moderate_completed}/5 sessions without a drop (required 5/5).", if rs_sessions_meet_acceptance(moderate_completed) { "MET" } else { "NOT MET" });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rs_moderate_requires_all_five_sessions() {
        assert!(!rs_sessions_meet_acceptance(4));
        assert!(rs_sessions_meet_acceptance(5));
    }

    #[test]
    fn good_baseline_meets_acceptance() {
        assert!(good_sessions_meet_acceptance(MIN_GOOD_COMPLETIONS));
    }

    #[test]
    fn fewer_than_good_baseline_is_a_regression() {
        assert!(!good_sessions_meet_acceptance(MIN_GOOD_COMPLETIONS - 1));
    }

    #[test]
    fn improvements_above_good_baseline_continue_to_pass() {
        assert!(good_sessions_meet_acceptance(SESSIONS_PER_PRESET));
    }
}
