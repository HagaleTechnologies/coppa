//! Phase C integration tests: CoppaTransceiver loopback at all speed levels.

use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::speed_levels::max_payload_for_level;
use coppa_protocol::modem::CoppaTransceiver;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

/// Max application-payload capacity for a speed level: the NR BG2 mother code's
/// shortened `k_used` info width for that level (Task 4), in bytes, minus the
/// 4-byte CRC-32 trailer `CoppaTransceiver::transmit` appends (Phase 3 Task 1) --
/// see `max_payload_for_level`'s doc. Was `code_rate.info_bits()/8` pre-Task-4
/// (the old per-rate 802.11 QC-LDPC codec's info width, which for level 10 was
/// 1701 bits/7/8 rate; the new mother code's level 10 is 1620 bits/5/6 rate -- a
/// wire-format break, see `CLAUDE.md`'s Known Limitations and this task's ADR).
fn max_payload_bytes(wire_level: u8) -> usize {
    max_payload_for_level(wire_level).unwrap()
}

fn make_header(speed_level: u8, payload_len: u16) -> CoppaHeader {
    CoppaHeader {
        version: 1,
        phy_mode: 0,
        frame_type: CoppaFrameType::Data,
        bandwidth: 1,
        fec_type: 0,
        speed_level,
        seq_num: 42,
        payload_len,
        codewords: 1,
    }
}

fn loopback_test(wire_level: u8, payload: &[u8]) {
    let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
    let header = make_header(wire_level, payload.len() as u16);

    let samples = transceiver
        .transmit(&header, payload)
        .expect("payload within this level's capacity");
    assert!(
        !samples.is_empty(),
        "Level {}: transmit produced no samples",
        wire_level
    );

    let (rx_header, rx_payload, _rec_level) = transceiver
        .receive(&samples)
        .unwrap_or_else(|e| panic!("Level {}: receive failed: {}", wire_level, e));

    assert_eq!(
        rx_header.version, header.version,
        "Level {}: version mismatch",
        wire_level
    );
    assert_eq!(
        rx_header.speed_level, header.speed_level,
        "Level {}: speed_level mismatch",
        wire_level
    );
    assert_eq!(
        rx_header.seq_num, header.seq_num,
        "Level {}: seq_num mismatch",
        wire_level
    );
    assert_eq!(
        rx_header.payload_len, header.payload_len,
        "Level {}: payload_len mismatch",
        wire_level
    );
    assert_eq!(
        &rx_payload[..payload.len()],
        payload,
        "Level {}: payload mismatch",
        wire_level
    );
}

#[test]
fn test_level_1_bpsk_rate_1_4() {
    loopback_test(1, b"BPSK quarter rate");
}

#[test]
fn test_level_2_bpsk_rate_1_2() {
    loopback_test(2, b"BPSK half rate test payload");
}

#[test]
fn test_level_3_qpsk_rate_1_2() {
    loopback_test(3, &[0xAB; 50]);
}

#[test]
fn test_level_4_qpsk_rate_3_4() {
    loopback_test(4, &[0xCD; 80]);
}

#[test]
fn test_level_5_8psk_rate_2_3() {
    loopback_test(5, &[0xEF; 60]);
}

#[test]
fn test_level_6_16qam_rate_1_2() {
    loopback_test(6, &[0x12; 50]);
}

#[test]
fn test_level_7_16qam_rate_3_4() {
    loopback_test(7, &[0x34; 100]);
}

#[test]
fn test_level_9_64qam_rate_2_3() {
    // The #[ignore] this test used to carry (routing around the old
    // per-rate LDPC codec's level-9/10 non-convergence) is removed: Task 4's
    // NR BG2 mother code fixes this — verified via a fresh
    // test_snr_fer_monte_carlo run showing FER=0.00 at every level 1-10.
    loopback_test(9, &[0x56; 80]);
}

#[test]
fn test_level_10_64qam_rate_5_6() {
    // Renamed from `rate_7_8`: Task 4's k_used table moved level 10 from
    // 7/8 to 5/6 (wire-format break, see CLAUDE.md's Known Limitations).
    // The #[ignore] main previously had here (routing around the old
    // per-rate LDPC codec's level-9/10 non-convergence) is removed: Task 4's
    // NR BG2 mother code fixes this — verified via a fresh
    // test_snr_fer_monte_carlo run showing FER=0.00 at every level 1-10.
    loopback_test(10, &[0x78; 150]);
}

#[test]
fn test_all_levels_max_payload() {
    // The levels-9/10 skip this loop used to have (routing around the old
    // per-rate LDPC codec's non-convergence) is removed: Task 4's NR BG2
    // mother code fixes this — verified via a fresh test_snr_fer_monte_carlo
    // run showing FER=0.00 at every level 1-10.
    for sl in &SPEED_LEVELS {
        let max_bytes = max_payload_bytes(sl.level);
        let payload: Vec<u8> = (0..max_bytes).map(|i| (i & 0xFF) as u8).collect();
        loopback_test(sl.level, &payload);
    }
}

#[test]
fn test_all_levels_min_payload() {
    // The levels-9/10 skip this loop used to have (routing around the old
    // per-rate LDPC codec's non-convergence) is removed: Task 4's NR BG2
    // mother code fixes this — verified via a fresh test_snr_fer_monte_carlo
    // run showing FER=0.00 at every level 1-10.
    for sl in &SPEED_LEVELS {
        loopback_test(sl.level, &[0x42]);
    }
}

#[test]
fn test_header_fields_roundtrip() {
    let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
    let header = CoppaHeader {
        version: 1,
        phy_mode: 2,
        frame_type: CoppaFrameType::Beacon,
        bandwidth: 1,
        fec_type: 0,
        speed_level: 3,
        seq_num: 255,
        payload_len: 10,
        codewords: 1,
    };
    let payload = vec![0xFF; 10];

    let samples = transceiver
        .transmit(&header, &payload)
        .expect("payload within this level's capacity");
    let (rx_header, _, _) = transceiver.receive(&samples).expect("should decode");

    assert_eq!(rx_header.version, 1);
    assert_eq!(rx_header.phy_mode, 2);
    assert_eq!(rx_header.frame_type, CoppaFrameType::Beacon);
    assert_eq!(rx_header.bandwidth, 1);
    assert_eq!(rx_header.fec_type, 0);
    assert_eq!(rx_header.speed_level, 3);
    assert_eq!(rx_header.seq_num, 255);
    assert_eq!(rx_header.payload_len, 10);
}

/// Add AWGN noise to samples at a given SNR (dB).
fn add_awgn(samples: &[f32], snr_db: f32, seed: u64) -> Vec<f32> {
    let signal_power: f32 = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let noise_power = signal_power / 10.0f32.powf(snr_db / 10.0);
    let noise_std = noise_power.sqrt();

    let mut rng = StdRng::seed_from_u64(seed);
    samples
        .iter()
        .map(|&s| {
            let u1: f32 = rng.random::<f32>().max(1e-10);
            let u2: f32 = rng.random();
            let noise =
                noise_std * (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
            s + noise
        })
        .collect()
}

/// Required SNR (dB) for each wire-encoded speed level.
fn required_snr(wire_level: u8) -> f32 {
    match wire_level {
        1 => 0.0,
        2 => 2.0,
        3 => 4.0,
        4 => 7.0,
        5 => 9.0,
        6 => 9.0,
        7 => 13.0,
        9 => 18.0,
        10 => 22.0,
        _ => 30.0,
    }
}

/// Test that a speed level decodes reliably well above threshold.
fn awgn_above_threshold(wire_level: u8) {
    let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
    let max_bytes = max_payload_bytes(wire_level).min(50);
    let payload: Vec<u8> = (0..max_bytes).map(|i| (i & 0xFF) as u8).collect();
    let header = make_header(wire_level, payload.len() as u16);

    let snr = required_snr(wire_level) + 6.0;
    let num_frames = 20;
    let mut failures = 0;

    for seed in 0..num_frames {
        let samples = transceiver
            .transmit(&header, &payload)
            .expect("payload within this level's capacity");
        let noisy = add_awgn(&samples, snr, seed);
        match transceiver.receive(&noisy) {
            Ok((_, rx_payload, _)) => {
                if rx_payload[..payload.len()] != payload[..] {
                    failures += 1;
                }
            }
            Err(_) => failures += 1,
        }
    }

    assert_eq!(
        failures, 0,
        "Level {} at SNR={:.0}dB: {}/{} frames failed (expected 0)",
        wire_level, snr, failures, num_frames
    );
}

/// Test that a speed level fails gracefully below threshold.
fn awgn_below_threshold(wire_level: u8) {
    let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
    let payload = vec![0x42u8; 20];
    let header = make_header(wire_level, payload.len() as u16);

    let snr = required_snr(wire_level) - 6.0;
    let samples = transceiver
        .transmit(&header, &payload)
        .expect("payload within this level's capacity");
    let noisy = add_awgn(&samples, snr, 999);

    // Should either fail to decode or produce wrong payload — both acceptable.
    // Key assertion: doesn't panic.
    let _ = transceiver.receive(&noisy);
}

#[test]
fn test_awgn_level_1_above_threshold() {
    awgn_above_threshold(1);
}

#[test]
fn test_awgn_level_2_above_threshold() {
    awgn_above_threshold(2);
}

#[test]
fn test_awgn_level_3_above_threshold() {
    awgn_above_threshold(3);
}

#[test]
fn test_awgn_level_4_above_threshold() {
    awgn_above_threshold(4);
}

#[test]
fn test_awgn_level_5_above_threshold() {
    awgn_above_threshold(5);
}

#[test]
fn test_awgn_level_6_above_threshold() {
    awgn_above_threshold(6);
}

#[test]
fn test_awgn_level_7_above_threshold() {
    awgn_above_threshold(7);
}

#[test]
fn test_awgn_level_9_above_threshold() {
    // The #[ignore] this test used to carry (routing around the old
    // per-rate LDPC codec's level-9/10 non-convergence) is removed: Task 4's
    // NR BG2 mother code fixes this — verified via a fresh
    // test_snr_fer_monte_carlo run showing FER=0.00 at every level 1-10.
    awgn_above_threshold(9);
}

#[test]
fn test_awgn_level_10_above_threshold() {
    // The #[ignore] this test used to carry (routing around the old
    // per-rate LDPC codec's level-9/10 non-convergence) is removed: Task 4's
    // NR BG2 mother code fixes this — verified via a fresh
    // test_snr_fer_monte_carlo run showing FER=0.00 at every level 1-10.
    awgn_above_threshold(10);
}

#[test]
fn test_awgn_below_threshold_no_panic() {
    for sl in &SPEED_LEVELS {
        awgn_below_threshold(sl.level);
    }
}

#[test]
#[ignore = "Monte Carlo FER sweep: ~7200 frames, run manually with --ignored --nocapture"]
fn test_snr_fer_monte_carlo() {
    let transceiver = CoppaTransceiver::new(CoppaProfile::hf_standard(), 1);
    let num_frames: u64 = 100;

    println!(
        "\n=== SNR vs FER Monte Carlo ({} frames/point) ===\n",
        num_frames
    );

    for sl in &SPEED_LEVELS {
        let max_bytes = max_payload_bytes(sl.level).min(50);
        let payload: Vec<u8> = (0..max_bytes).map(|i| (i & 0xFF) as u8).collect();
        let header = make_header(sl.level, payload.len() as u16);

        let base_snr = required_snr(sl.level);
        let snr_start = (base_snr - 4.0).max(-2.0);
        let snr_end = base_snr + 10.0;
        let snr_step = 2.0f32;

        println!(
            "Level {:>2} (bps={}, rate={}/{}):",
            sl.level, sl.bits_per_symbol, sl.ldpc_rate_num, sl.ldpc_rate_den
        );

        let mut snr = snr_start;
        while snr <= snr_end + 0.01 {
            let mut errors = 0u64;
            for seed in 0..num_frames {
                let samples = transceiver
                    .transmit(&header, &payload)
                    .expect("payload within this level's capacity");
                let noisy = add_awgn(&samples, snr, seed);
                match transceiver.receive(&noisy) {
                    Ok((_, rx_payload, _)) => {
                        if rx_payload[..payload.len()] != payload[..] {
                            errors += 1;
                        }
                    }
                    Err(_) => errors += 1,
                }
            }
            let fer = errors as f64 / num_frames as f64;
            println!(
                "  SNR={:6.1}dB  FER={:.2} ({}/{})",
                snr, fer, errors, num_frames
            );
            snr += snr_step;
        }
        println!();
    }
}
