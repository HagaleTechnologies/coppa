//! Audit the development evidence without treating it as production acceptance.
use coppa_bench::floor_sweep::{acceptance_grid, evaluate, PointTally, CSV_HEADER, DEV_SEED_BASE};
use std::{fs, path::Path};

fn audit(directory: &str, channel: &str) -> Vec<PointTally> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../results/cop-187")
        .join(directory);
    let metadata = fs::read_to_string(root.join("metadata.txt")).unwrap();
    for expected in [
        "production=false".to_owned(),
        "acceptance_run=false".to_owned(),
        format!("seed_base={DEV_SEED_BASE}"),
        "trials_per_point=10".to_owned(),
        format!("channel={channel}"),
        "frame_samples=384000".to_owned(),
    ] {
        assert!(metadata.lines().any(|line| line == expected), "{expected}");
    }
    let csv = fs::read_to_string(root.join("sweep.csv")).unwrap();
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some(CSV_HEADER));
    let mut points = Vec::new();
    for line in lines {
        let fields: Vec<_> = line.split(',').collect();
        assert_eq!(fields.len(), CSV_HEADER.split(',').count());
        let count = |index: usize| fields[index].parse::<usize>().unwrap();
        let errors = count(5);
        let trials = count(4);
        let point = PointTally {
            snr_db: fields[3].parse().unwrap(),
            trials,
            delivered: trials.checked_sub(errors).unwrap(),
            sync_failed: count(6),
            sync_no_detection: count(7),
            sync_mistimed: count(8),
            sync_header_decode: count(9),
            header_corrupt: count(10),
            payload_failed: count(11),
            wrong_payload: count(12),
            airtime_samples: trials as u64 * 384000,
            delivered_bytes: (trials - errors) as u64 * 40,
        };
        assert_eq!(trials, 10);
        assert!(point.is_partition());
        assert_eq!(point.frame_errors(), errors);
        assert_eq!(point.wrong_payload, 0);
        // Recompute FER, Wilson limits, airtime and goodput, including rounding.
        assert_eq!(point.csv_row("experimental-mfsk16", "batch", channel), line);
        points.push(point);
    }
    assert_eq!(
        points
            .iter()
            .map(|point| point.snr_db as i32)
            .collect::<Vec<_>>(),
        acceptance_grid()
    );
    let verdict = evaluate(&points, if channel == "poor" { -2.0 } else { -8.0 });
    assert!(metadata
        .lines()
        .any(|line| line == format!("verdict={verdict:?}")));
    assert!(metadata
        .lines()
        .any(|line| line == format!("passes={}", verdict.passes())));
    points
}

#[test]
fn development_header_comparison_is_auditable_and_still_not_acceptance() {
    let before = audit("dev-header-before-poor", "poor");
    let after = audit("dev-header-poor", "poor");
    let errors = |points: &[PointTally]| points.iter().map(PointTally::frame_errors).sum::<usize>();
    assert!(errors(&after) < errors(&before));
    // The residual acquisition failures keep this candidate unselected.
    assert!(!evaluate(&after, -2.0).passes());
}

#[test]
fn development_awgn_check_is_auditable() {
    let points = audit("dev-header-awgn", "awgn");
    assert_eq!(
        points.iter().map(PointTally::frame_errors).sum::<usize>(),
        1
    );
    assert_eq!(points[0].payload_failed, 1);
    assert!(evaluate(&points, -8.0).passes());
}

#[test]
fn cop191_final_sweeps_recompute() {
    use coppa_bench::floor_sweep::{SweepChannel, FINAL_SWEEP_SEED_BASE, TRIALS_PER_POINT};
    use coppa_protocol::modem::floor::{FRAME_SAMPLES, MAX_PAYLOAD};
    for channel in [SweepChannel::Awgn, SweepChannel::Poor] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../results/cop-191")
            .join(format!("final-{}", channel.name()));
        let metadata = fs::read_to_string(root.join("metadata.txt")).unwrap();
        for expected in [
            "waveform=floor".to_owned(),
            "production=true".to_owned(),
            "complete=true".to_owned(),
            "acceptance_run=true".to_owned(),
            format!("seed_base={FINAL_SWEEP_SEED_BASE:#x}"),
            format!("trials_per_point={TRIALS_PER_POINT}"),
            "rx_paths=[\"batch\", \"streaming\"]".to_owned(),
            "tail_s=1.25".to_owned(),
            "stream_chunk_samples=4800".to_owned(),
            "snr_reference=clean frame mean power, 3 kHz".to_owned(),
        ] {
            assert!(metadata.lines().any(|line| line == expected), "{expected}");
        }
        assert!(metadata.contains(
            "seed_rule=attempt_seed(base, point, trials, trial) = base + point*trials + trial"
        ));
        assert!(metadata.contains("invocation="));
        let csv = fs::read_to_string(root.join("sweep.csv")).unwrap();
        let mut lines = csv.lines();
        assert_eq!(lines.next(), Some(CSV_HEADER));
        let rows: Vec<_> = lines.collect();
        assert_eq!(rows.len(), 18);
        let mut passes = Vec::new();
        for (path, rows) in ["batch", "streaming"].into_iter().zip(rows.chunks(9)) {
            let mut points = Vec::new();
            for (&snr, &line) in acceptance_grid().iter().zip(rows) {
                let fields: Vec<_> = line.split(',').collect();
                assert_eq!(fields.len(), CSV_HEADER.split(',').count());
                assert_eq!(&fields[..3], &["floor", path, channel.name()]);
                let count = |i: usize| fields[i].parse::<usize>().unwrap();
                let errors = count(5);
                let trials = count(4);
                assert_eq!(trials, TRIALS_PER_POINT);
                let point = PointTally {
                    snr_db: fields[3].parse().unwrap(),
                    trials,
                    delivered: trials.checked_sub(errors).unwrap(),
                    sync_failed: count(6),
                    sync_no_detection: count(7),
                    sync_mistimed: count(8),
                    sync_header_decode: count(9),
                    header_corrupt: count(10),
                    payload_failed: count(11),
                    wrong_payload: count(12),
                    airtime_samples: (trials * FRAME_SAMPLES) as u64,
                    delivered_bytes: ((trials - errors) * MAX_PAYLOAD) as u64,
                };
                assert_eq!(point.snr_db, f64::from(snr));
                assert!(point.is_partition());
                assert_eq!(point.frame_errors(), errors);
                assert_eq!(point.csv_row("floor", path, channel.name()), line);
                points.push(point);
            }
            let verdict = evaluate(&points, channel.bar_db());
            let block = metadata.split(&format!("rx_path={path}\n")).nth(1).unwrap();
            for expected in [
                format!("crossing={:?}", verdict.crossing),
                format!("sync_share_violations={:?}", verdict.sync_share_violations),
                format!("wrong_payloads={}", verdict.wrong_payloads),
                format!("passes={}", verdict.passes()),
            ] {
                assert!(
                    block
                        .split("rx_path=")
                        .next()
                        .unwrap()
                        .lines()
                        .any(|l| l == expected),
                    "{expected}"
                );
            }
            passes.push(verdict.passes());
        }
        assert!(metadata
            .lines()
            .any(|line| line == format!("all_selected_pass={}", passes.iter().all(|p| *p))));
    }
}

#[test]
fn integer_tone_cfo_development_measurements_are_auditable() {
    for (directory, cfo) in [
        ("dev-cfo-zero-poor", 0),
        ("dev-cfo-minus150-poor", -150),
        ("dev-cfo-plus150-poor", 150),
    ] {
        let points = audit(directory, "poor");
        let metadata = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../results/cop-187")
                .join(directory)
                .join("metadata.txt"),
        )
        .unwrap();
        for expected in [
            format!("cfo_hz={cfo}"),
            "rx=bounded-batch-integer-tone-cfo-search".to_owned(),
            "cfo_search_hz=-150:50:150".to_owned(),
            "max_decode_candidates=16".to_owned(),
        ] {
            assert!(metadata.lines().any(|line| line == expected), "{expected}");
        }
        // These development data must not be promoted to final acceptance:
        // only ten trials per point, no streaming receiver or session caller.
        assert!(points.iter().all(|point| point.wrong_payload == 0));
    }
}
