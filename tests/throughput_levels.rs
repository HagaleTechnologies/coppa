#[path = "../benches/support/engine_cases.rs"]
mod engine_cases;

use coppa_engine::ladder::speed_ladder;
use coppa_protocol::cp_negotiator::CpMode;
use engine_cases::{frame_metrics, prepare_cases, PAYLOAD};

#[test]
fn fixtures_cover_every_level_with_verified_common_payload() {
    let cases = prepare_cases().expect("all levels must recover the benchmark payload");
    let levels: Vec<_> = cases.iter().map(|case| case.level).collect();
    assert_eq!(
        levels,
        speed_ladder()
            .iter()
            .map(|row| row.level)
            .collect::<Vec<_>>()
    );
    assert_eq!(levels, (1..=9).collect::<Vec<_>>());
    assert_eq!(PAYLOAD, "0123456789ABCDEF0123456789ABCDEF");
    assert_eq!(PAYLOAD.len(), 32);
    for (case, row) in cases.iter().zip(speed_ladder()) {
        assert_eq!(case.core.decode(&case.samples).unwrap(), PAYLOAD);
        assert!(!case.core.config().compression_enabled);
        assert_eq!(case.core.config().sample_rate, 48_000);
        assert_eq!(case.core.config().cp_mode, CpMode::LongCp);
        assert_eq!(case.core.config().speed_level, case.level);
        assert_eq!(case.core.on_vhf_profile(), case.level >= 5);
        assert_eq!(
            case.profile,
            if case.core.on_vhf_profile() {
                "vhf_wide"
            } else {
                "hf_standard"
            }
        );
        assert!(PAYLOAD.len() <= row.max_payload_bytes);
        let expected = frame_metrics(
            case.samples.len(),
            case.core.config().sample_rate,
            PAYLOAD.len(),
        )
        .unwrap();
        assert_eq!(case.metrics, expected);
    }
}

#[test]
fn metric_arithmetic_uses_audio_airtime_and_rejects_zero_denominators() {
    let metrics = frame_metrics(48_000, 48_000, 32).unwrap();
    assert_eq!(metrics.airtime_s, 1.0);
    assert_eq!(metrics.goodput_bps, 256.0);
    assert!(frame_metrics(0, 48_000, 32).is_err());
    assert!(frame_metrics(48_000, 0, 32).is_err());
}

#[test]
fn report_has_nine_ordered_rows_with_actual_frame_metrics() {
    let cases = prepare_cases().unwrap();
    let report = engine_cases::frame_report(&cases);
    let lines: Vec<_> = report.lines().collect();
    assert_eq!(
        &lines[..4],
        &[
            "Engine frame metrics: ideal clean loopback",
            "Payload: 32 bytes; compression: off; sample rate: 48000 Hz; CP: long",
            "Goodput excludes channel losses, ARQ, turnaround and session overhead.",
            "level,profile,payload_bytes,frame_samples,frame_airtime_s,ideal_frame_goodput_bps",
        ]
    );
    assert_eq!(lines.len(), 13);
    for (line, case) in lines[4..].iter().zip(&cases) {
        let fields: Vec<_> = line.split(',').collect();
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[0].parse::<u8>().unwrap(), case.level);
        assert_eq!(fields[1], case.profile);
        assert_eq!(fields[2], "32");
        assert_eq!(fields[3].parse::<usize>().unwrap(), case.samples.len());
        let airtime = fields[4].parse::<f64>().unwrap();
        let goodput = fields[5].parse::<f64>().unwrap();
        assert!(airtime.is_finite() && goodput.is_finite());
        assert!((airtime - case.samples.len() as f64 / 48_000.0).abs() <= 0.0000005);
        assert!((goodput - 256.0 / case.metrics.airtime_s).abs() <= 0.005);
    }
}
