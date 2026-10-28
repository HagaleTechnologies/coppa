//! Phase C integration tests: CoppaTransceiver loopback at all speed levels.

use coppa_bench::runner::run_scenario;
use coppa_bench::scenario::{ChannelSpec, Scenario};
use coppa_codec::ofdm::coppa_modem::SPEED_LEVELS;
use coppa_codec::ofdm::frame::{CoppaFrameType, CoppaHeader};
use coppa_codec::ofdm::CoppaProfile;
use coppa_protocol::modem::speed_levels::max_payload_for_level;
use coppa_protocol::modem::CoppaTransceiver;

/// Max application-payload capacity for a speed level: the NR BG2 mother code's
/// shortened `k_used` info width for that level (Task 4), in bytes, minus the
/// 4-byte CRC-32 trailer `CoppaTransceiver::transmit` appends (Phase 3 Task 1) --
/// see `max_payload_for_level`'s doc. Was `code_rate.info_bits()/8` pre-Task-4
/// (the old per-rate 802.11 QC-LDPC codec's info width, which for level 10 was
/// 1701 bits/7/8 rate; the new mother code's level 9 is 1620 bits/5/6 rate -- a
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
fn test_level_8_64qam_rate_2_3() {
    // The #[ignore] this test used to carry (routing around the old
    // per-rate LDPC codec's level-9/10 non-convergence) is removed: Task 4's
    // NR BG2 mother code fixes this. The measured evidence is COP-24's
    // SSB-filtered AWGN sweep (BENCHMARKS.md), guarded on every push by
    // coppa-bench's `waterfall_bracket_level_*` tests -- not the old
    // `test_snr_fer_monte_carlo`, which could not fail (see its doc).
    loopback_test(8, &[0x56; 80]);
}

#[test]
fn test_level_9_64qam_rate_5_6() {
    // Renamed from `rate_7_8`: Task 4's k_used table moved level 10 from
    // 7/8 to 5/6 (wire-format break, see CLAUDE.md's Known Limitations).
    // The #[ignore] main previously had here (routing around the old
    // per-rate LDPC codec's level-9/10 non-convergence) is removed: Task 4's
    // NR BG2 mother code fixes this on the unfiltered AWGN channel, guarded
    // on every push by coppa-bench's `waterfall_bracket_level_9` -- not the
    // old `test_snr_fer_monte_carlo`, which could not fail (see its doc).
    // Through the SSB filter level 9 still never clears 10% FER
    // (BENCHMARKS.md's COP-24 section), which is why it is VHF-only.
    loopback_test(9, &[0x78; 150]);
}

#[test]
fn test_all_levels_max_payload() {
    // The levels-9/10 skip this loop used to have (routing around the old
    // per-rate LDPC codec's non-convergence) is removed: Task 4's NR BG2
    // mother code fixes this, per COP-24's SSB-filtered AWGN sweep
    // (BENCHMARKS.md) and coppa-bench's `waterfall_bracket_level_*` tests.
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
    // mother code fixes this, per COP-24's SSB-filtered AWGN sweep
    // (BENCHMARKS.md) and coppa-bench's `waterfall_bracket_level_*` tests.
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

/// Manual full AWGN FER/goodput sweep over every speed level (COP-70).
///
/// Drives `coppa_bench::runner::run_scenario`, the harness behind every
/// BENCHMARKS.md figure: 3 kHz-referenced noise (`awgn_ref_seeded`) against
/// the clean signal's power, a fresh random payload per trial, and the
/// IR-HARQ buffer evicted after every trial. Each level's window straddles
/// its measured waterfall, so the printed rows show the cliff with Wilson
/// bounds and goodput in the same convention as BENCHMARKS.md.
///
/// This replaces a sweep that could not fail: it added full-band noise
/// (~9 dB optimistic against the 3 kHz convention), swept only above every
/// level's waterfall, re-sent one fixed frame without evicting its HARQ
/// buffer, and asserted nothing, so it read zero errors everywhere regardless
/// of real performance. The per-push guard is coppa-bench's
/// `waterfall_bracket_level_*` tests (`crates/coppa-bench/src/runner.rs`);
/// this is the human-run version that catches drift a 20-trial bracket
/// cannot. Every level is pinned to `hf_standard` to match those anchors,
/// which is not production routing for levels 5-9.
///
/// Each window's low edge must fail (FER Wilson lower bound >= 0.5) and its
/// high edge must clear FER<=10% on the Wilson upper bound. When a level
/// fails here, its waterfall has moved: re-measure it and re-record both
/// this window and the bracket anchors.
#[test]
#[ignore = "manual full AWGN FER sweep: 6000 frames, run with --release -- --ignored --nocapture"]
fn test_snr_fer_monte_carlo() {
    const TRIALS: usize = 100;
    // (level, window low edge dB, window high edge dB), 3 kHz convention.
    // Measured waterfalls: L1 4-6, L2/L3 ~4, L4 4-6, L5 8-10, L6 ~8,
    // L7 10-12, L8 14-16, L9 18-22 dB.
    const WINDOWS: [(u8, f32, f32); 9] = [
        (1, -2.0, 8.0),
        (2, -2.0, 8.0),
        (3, -2.0, 8.0),
        (4, -2.0, 8.0),
        (5, 0.0, 12.0),
        (6, 0.0, 12.0),
        (7, 2.0, 14.0),
        (8, 6.0, 18.0),
        (9, 10.0, 24.0),
    ];

    // Levels are independent, so run them concurrently and print in order.
    let sweeps: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = WINDOWS
            .iter()
            .map(|&(level, low_db, high_db)| {
                scope.spawn(move || {
                    let steps = ((high_db - low_db) / 2.0).round() as usize;
                    let scenario = Scenario {
                        level,
                        channel: ChannelSpec::Awgn,
                        snr_db_points: (0..=steps).map(|i| low_db + 2.0 * i as f32).collect(),
                        trials: TRIALS,
                        // Spaced so no two levels share a per-trial seed.
                        seed: 0x2710 + 10_000 * u64::from(level),
                        profile_override: Some(CoppaProfile::hf_standard()),
                        cfo_hz: 0.0,
                        sco_ppm: 0.0,
                        ssb: false,
                    };
                    run_scenario(&scenario)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("sweep thread panicked"))
            .collect()
    });

    println!(
        "\n=== AWGN FER sweep, hf_standard, 3 kHz noise bandwidth, {TRIALS} frames/point ===\n"
    );
    let mut misplaced = Vec::new();
    for points in &sweeps {
        let first = &points[0];
        println!("Level {} ({}):", first.level, first.mode_name);
        for p in points {
            let f = p.failures;
            println!(
                "  SNR={:6.1}dB  FER={:.3} ({:>3}/{}) Wilson95=[{:.3}, {:.3}]  goodput={:7.1} bps  \
                 sync={} header={} ldpc={} crc={} wrong={}",
                p.snr_db,
                p.fer,
                p.frame_errors,
                p.trials,
                p.fer_lo,
                p.fer_hi,
                p.goodput_bps,
                f.sync_failed,
                f.header_corrupt,
                f.ldpc_not_converged,
                f.crc_mismatch,
                f.wrong_payload,
            );
        }
        println!();

        let last = points.last().expect("every window has at least one point");
        if first.fer_lo < 0.5 {
            misplaced.push(format!(
                "level {} low edge {} dB: FER Wilson lower bound {:.3} < 0.5",
                first.level, first.snr_db, first.fer_lo
            ));
        }
        if last.fer_hi > 0.10 {
            misplaced.push(format!(
                "level {} high edge {} dB: FER Wilson upper bound {:.3} > 0.10",
                last.level, last.snr_db, last.fer_hi
            ));
        }
    }
    assert!(
        misplaced.is_empty(),
        "sweep windows no longer straddle the measured waterfalls; re-measure \
         and re-record them and the coppa-bench bracket anchors:\n{}",
        misplaced.join("\n")
    );
}
