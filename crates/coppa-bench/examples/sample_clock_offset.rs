//! COP-3 diagnostic: signed sampling-clock offset over frames and simulated
//! ten-minute ARQ sessions.
//!
//! Run: `cargo run -p coppa-bench --release --example sample_clock_offset`

use std::time::{Duration, Instant};

use coppa_bench::metrics::wilson_ci95;
use coppa_bench::scenario::{mode_for_level, select_profile, SAMPLE_RATE};
use coppa_channel::watterson::WattersonPreset;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_protocol::arq::{ArqConfig, ArqRx, ArqTx};
use coppa_protocol::modem::frame_airtime_s;
use coppa_protocol::modem::transceiver::{CoppaTransceiver, ReceiveError};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

const LEVEL: u8 = 2;
const PPM_POINTS: [f32; 7] = [-120.0, -100.0, -50.0, 0.0, 50.0, 100.0, 120.0];
const FRAME_TRIALS: usize = 20;
const SESSION_DURATION: Duration = Duration::from_secs(600);
const SESSIONS_PER_CELL: usize = 1;
const WINDOW: u8 = 8;
const TURNAROUND: Duration = Duration::from_millis(150);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct OutcomeCounts {
    correct: usize,
    wrong_payload: usize,
    sync_failed: usize,
    header_corrupt: usize,
    ldpc_not_converged: usize,
    crc_mismatch: usize,
}

impl OutcomeCounts {
    fn record(&mut self, result: Result<bool, ReceiveError>) {
        match result {
            Ok(true) => self.correct += 1,
            Ok(false) => self.wrong_payload += 1,
            Err(ReceiveError::SyncFailed) => self.sync_failed += 1,
            Err(ReceiveError::HeaderCorrupt) => self.header_corrupt += 1,
            Err(ReceiveError::LdpcNotConverged { .. }) => self.ldpc_not_converged += 1,
            Err(ReceiveError::CrcMismatch) => self.crc_mismatch += 1,
        }
    }

    fn trials(self) -> usize {
        self.correct
            + self.wrong_payload
            + self.sync_failed
            + self.header_corrupt
            + self.ldpc_not_converged
            + self.crc_mismatch
    }

    fn frame_errors(self) -> usize {
        self.trials() - self.correct
    }

    fn fer(self) -> f64 {
        self.frame_errors() as f64 / self.trials().max(1) as f64
    }
}

#[derive(Debug, Clone, Copy)]
enum TestChannel {
    Awgn,
    Watterson(WattersonPreset),
}

impl TestChannel {
    fn name(self) -> &'static str {
        match self {
            Self::Awgn => "awgn",
            Self::Watterson(WattersonPreset::Good) => "watterson-good",
            Self::Watterson(WattersonPreset::Moderate) => "watterson-moderate",
            Self::Watterson(WattersonPreset::Poor) => "watterson-poor",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct SessionSummary {
    dropped: bool,
    elapsed_s: f64,
    bytes_delivered: usize,
    retransmissions: usize,
}

fn make_header(payload_len: usize, seq: u8) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level: LEVEL,
        seq_num: seq,
        payload_len: payload_len as u16,
        codewords: 1,
    }
}

fn paired_seed(base: u64, trial: usize) -> u64 {
    base.wrapping_add(trial as u64)
}

fn apply_channel(
    clean: &[f32],
    channel: TestChannel,
    snr_db: f32,
    ppm: f32,
    seed: u64,
) -> Vec<f32> {
    let sr = SAMPLE_RATE as f32;
    let clean_power = coppa_channel::mean_power(clean);
    let faded = match channel {
        TestChannel::Awgn => clean.to_vec(),
        TestChannel::Watterson(preset) => coppa_channel::watterson::watterson_preset(
            clean,
            sr,
            preset,
            seed ^ 0x3333_3333_3333_3333,
        ),
    };
    let noisy = coppa_channel::awgn_ref_seeded(
        &faded,
        snr_db,
        clean_power,
        sr,
        seed ^ 0x5555_5555_5555_5555,
    );
    coppa_channel::sample_clock_offset(&noisy, ppm).expect("diagnostic ppm is valid")
}

fn transmit_once(
    phy: &CoppaTransceiver,
    seq: u8,
    data: &[u8],
    channel: TestChannel,
    snr_db: f32,
    ppm: f32,
    seed: u64,
) -> (Result<bool, ReceiveError>, usize) {
    let header = make_header(data.len(), seq);
    let clean = phy.transmit(&header, data).expect("level payload fits");
    let tx_samples = clean.len();
    let received = apply_channel(&clean, channel, snr_db, ppm, seed);
    let result = phy
        .receive(&received)
        .map(|(_, payload, _)| payload.len() >= data.len() && payload[..data.len()] == data[..]);
    phy.harq_evict(seq);
    (result, tx_samples)
}

fn run_frame_cell(channel: TestChannel, snr_db: f32, ppm: f32) -> OutcomeCounts {
    let phy = CoppaTransceiver::new(select_profile(LEVEL), 1);
    let payload_bytes = mode_for_level(LEVEL).unwrap().payload_bytes();
    let mut counts = OutcomeCounts::default();
    for trial in 0..FRAME_TRIALS {
        let seed = paired_seed(0xC0_0003, trial);
        let mut rng = StdRng::seed_from_u64(seed);
        let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.random()).collect();
        let (outcome, _) = transmit_once(&phy, 0, &payload, channel, snr_db, ppm, seed);
        counts.record(outcome);
    }
    counts
}

fn ramp_snr_db(elapsed_s: f64, total_s: f64) -> f32 {
    let half = total_s / 2.0;
    if elapsed_s <= half {
        (20.0 - 20.0 * elapsed_s / half) as f32
    } else {
        (20.0 * ((elapsed_s - half).min(half) / half)) as f32
    }
}

fn run_session(preset: WattersonPreset, ppm: f32, seed: u64, duration: Duration) -> SessionSummary {
    let payload_bytes = mode_for_level(LEVEL).unwrap().payload_bytes();
    let profile = select_profile(LEVEL);
    let phy = CoppaTransceiver::new(profile.clone(), 1);
    let ack_airtime = Duration::from_secs_f64(frame_airtime_s(LEVEL, &profile).unwrap());
    let config = ArqConfig::new(WINDOW, 5, Duration::from_secs(5))
        .unwrap()
        .with_airtime_params(LEVEL, TURNAROUND, profile);
    let mut tx = ArqTx::new(config);
    let mut rx = ArqRx::new(WINDOW);
    let start = Instant::now();
    let mut now = start;
    let mut summary = SessionSummary::default();
    let mut frame_no = 0u64;

    while now.duration_since(start) < duration && !summary.dropped {
        let snr = ramp_snr_db(
            now.duration_since(start).as_secs_f64(),
            duration.as_secs_f64(),
        );
        if tx.can_send() {
            let frame_seed = seed ^ frame_no ^ 0xACE0_0000;
            frame_no += 1;
            let mut rng = StdRng::seed_from_u64(frame_seed);
            let data: Vec<u8> = (0..payload_bytes).map(|_| rng.random()).collect();
            let seq = tx.send(data.clone(), now).unwrap();
            let (outcome, tx_samples) = transmit_once(
                &phy,
                seq,
                &data,
                TestChannel::Watterson(preset),
                snr,
                ppm,
                frame_seed,
            );
            now += Duration::from_secs_f64(tx_samples as f64 / SAMPLE_RATE as f64);
            if matches!(outcome, Ok(true)) {
                summary.bytes_delivered += rx
                    .receive(seq, data)
                    .iter()
                    .map(|(_, d)| d.len())
                    .sum::<usize>();
                now += TURNAROUND;
                let (ack, bitmap) = rx.ack_info();
                now += ack_airtime + TURNAROUND;
                tx.process_ack(ack, bitmap, now);
            }
        } else {
            now += Duration::from_millis(500);
        }

        for seq in tx.get_retransmits(now) {
            tx.mark_retransmitted(seq, now).unwrap();
            summary.retransmissions += 1;
            if tx.is_failed(seq) {
                summary.dropped = true;
                break;
            }
            let data = tx.get_segment_data(seq).unwrap().to_vec();
            let retry_seed = seed ^ u64::from(seq) ^ (summary.retransmissions as u64) << 32;
            let snr = ramp_snr_db(
                now.duration_since(start).as_secs_f64(),
                duration.as_secs_f64(),
            );
            let (outcome, tx_samples) = transmit_once(
                &phy,
                seq,
                &data,
                TestChannel::Watterson(preset),
                snr,
                ppm,
                retry_seed,
            );
            now += Duration::from_secs_f64(tx_samples as f64 / SAMPLE_RATE as f64);
            if matches!(outcome, Ok(true)) {
                summary.bytes_delivered += rx
                    .receive(seq, data)
                    .iter()
                    .map(|(_, d)| d.len())
                    .sum::<usize>();
                now += TURNAROUND;
                let (ack, bitmap) = rx.ack_info();
                now += ack_airtime + TURNAROUND;
                tx.process_ack(ack, bitmap, now);
            }
        }
    }
    summary.elapsed_s = now.duration_since(start).as_secs_f64();
    summary
}

fn main() {
    println!("=== COP-3 sample-clock-offset diagnostic ===");
    println!("level={LEVEL} profile=hf-standard scale=1+ppm/1e6; positive ppm shortens RX buffer");
    println!("frame_trials={FRAME_TRIALS} paired_seed_base=0xC00003 clean-referenced AWGN");
    println!("channel,snr_db,ppm,trials,correct,fer,ci_lo,ci_hi,wrong,sync,header,ldpc,crc");
    let frame_channels = [
        (TestChannel::Awgn, 30.0),
        (TestChannel::Watterson(WattersonPreset::Good), 18.0),
        (TestChannel::Watterson(WattersonPreset::Moderate), 24.0),
        (TestChannel::Watterson(WattersonPreset::Poor), 30.0),
    ];
    for (channel, snr) in frame_channels {
        for ppm in PPM_POINTS {
            let c = run_frame_cell(channel, snr, ppm);
            let (lo, hi) = wilson_ci95(c.frame_errors(), c.trials());
            println!(
                "{},{snr:.1},{ppm:+.0},{},{},{:.3},{lo:.3},{hi:.3},{},{},{},{},{}",
                channel.name(),
                c.trials(),
                c.correct,
                c.fer(),
                c.wrong_payload,
                c.sync_failed,
                c.header_corrupt,
                c.ldpc_not_converged,
                c.crc_mismatch
            );
        }
    }

    println!(
        "\nsession_duration_s={} sessions_per_cell={SESSIONS_PER_CELL} snr_ramp=20->0->20",
        SESSION_DURATION.as_secs()
    );
    println!("preset,ppm,completed,drops,retransmissions,bytes,bytes_per_min");
    for preset in [
        WattersonPreset::Good,
        WattersonPreset::Moderate,
        WattersonPreset::Poor,
    ] {
        for ppm in PPM_POINTS {
            let mut completed = 0;
            let mut retransmissions = 0;
            let mut bytes = 0;
            let mut elapsed = 0.0;
            for trial in 0..SESSIONS_PER_CELL {
                let result = run_session(
                    preset,
                    ppm,
                    paired_seed(0x5C05_5100 ^ (preset as u64) << 16, trial),
                    SESSION_DURATION,
                );
                completed += usize::from(!result.dropped);
                retransmissions += result.retransmissions;
                bytes += result.bytes_delivered;
                elapsed += result.elapsed_s;
            }
            let name = TestChannel::Watterson(preset).name();
            let drops = SESSIONS_PER_CELL - completed;
            let bpm = bytes as f64 / (elapsed / 60.0);
            println!("{name},{ppm:+.0},{completed},{drops},{retransmissions},{bytes},{bpm:.1}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_categories_and_fer_are_complete() {
        let mut counts = OutcomeCounts::default();
        for result in [
            Ok(true),
            Ok(false),
            Err(ReceiveError::SyncFailed),
            Err(ReceiveError::HeaderCorrupt),
            Err(ReceiveError::LdpcNotConverged { iterations: 4 }),
            Err(ReceiveError::CrcMismatch),
        ] {
            counts.record(result);
        }
        assert_eq!(counts.trials(), 6);
        assert_eq!(counts.frame_errors(), 5);
        assert!((counts.fer() - 5.0 / 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ppm_order_and_paired_seeds_are_stable() {
        assert_eq!(PPM_POINTS, [-120.0, -100.0, -50.0, 0.0, 50.0, 100.0, 120.0]);
        assert_eq!(paired_seed(10, 2), 12);
    }

    #[test]
    fn impairment_changes_receiver_length_but_not_transmitter_airtime_basis() {
        let clean = vec![0.0; 48_000];
        let positive = apply_channel(&clean, TestChannel::Awgn, 100.0, 120.0, 1);
        let negative = apply_channel(&clean, TestChannel::Awgn, 100.0, -120.0, 1);
        assert!(positive.len() < clean.len());
        assert!(negative.len() > clean.len());
        assert_eq!(
            Duration::from_secs_f64(clean.len() as f64 / SAMPLE_RATE as f64),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn short_session_summary_distinguishes_delivery_and_drop() {
        let result = run_session(WattersonPreset::Good, 0.0, 7, Duration::from_secs(1));
        assert!(result.elapsed_s >= 1.0);
        assert!(!result.dropped);
        assert!(result.bytes_delivered > 0);
    }
}
