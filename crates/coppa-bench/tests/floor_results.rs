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

#[test]
fn production_floor_development_measurements_are_auditable() {
    for channel in ["awgn", "poor"] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../results/cop-190")
            .join(format!("dev-{channel}"));
        let metadata = fs::read_to_string(root.join("metadata.txt")).unwrap();
        for expected in [
            "production=true".to_owned(),
            "acceptance_run=false".to_owned(),
            format!("seed_base={DEV_SEED_BASE}"),
            "trials_per_point=10".to_owned(),
            format!("channel={channel}"),
            "air_protocol_version=2".to_owned(),
            "frame_samples=384000".to_owned(),
        ] {
            assert!(metadata.lines().any(|line| line == expected), "{expected}");
        }
        let csv = fs::read_to_string(root.join("sweep.csv")).unwrap();
        let mut lines = csv.lines();
        assert_eq!(lines.next(), Some(CSV_HEADER));
        for rx in ["batch", "streaming"] {
            let mut points = Vec::new();
            for snr in acceptance_grid() {
                let line = lines.next().unwrap();
                let fields: Vec<_> = line.split(',').collect();
                assert_eq!(fields.len(), 18);
                assert_eq!(&fields[..3], &["floor-v2", rx, channel]);
                assert_eq!(fields[3].parse::<i32>().unwrap(), snr);
                let count = |index: usize| fields[index].parse::<usize>().unwrap();
                let trials = count(4);
                let errors = count(5);
                let point = PointTally {
                    snr_db: f64::from(snr),
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
                assert_eq!(point.csv_row("floor-v2", rx, channel), line);
                points.push(point);
            }
            let verdict = evaluate(&points, if channel == "poor" { -2.0 } else { -8.0 });
            assert!(metadata
                .lines()
                .any(|line| line == format!("{rx}_verdict={verdict:?}")));
        }
        assert!(lines.next().is_none());
    }
}

/// Reproduce the development evidence using the production floor receivers.
/// This writes only the two curated COP-190 result directories.
#[test]
#[ignore = "release-only production floor development sweep; writes results/cop-190"]
fn record_production_floor_development_sweeps() {
    use coppa_bench::floor_sweep::{attempt_seed, AttemptOutcome, SyncCause};
    use coppa_channel::persistent::{PersistentChannel, SnrProfile};
    use coppa_channel::watterson::WattersonPreset;
    use coppa_protocol::modem::floor::{FloorError, FloorFrame, FloorStream, FRAME_SAMPLES};
    use rand::{rngs::StdRng, RngExt, SeedableRng};

    let frame = FloorFrame::new();
    for channel in ["awgn", "poor"] {
        let started = std::time::Instant::now();
        let mut batch_points = Vec::new();
        let mut stream_points = Vec::new();
        for (point, snr) in acceptance_grid().into_iter().enumerate() {
            let mut batch = PointTally::new(f64::from(snr));
            let mut streaming = PointTally::new(f64::from(snr));
            for trial in 0..10 {
                let seed = attempt_seed(DEV_SEED_BASE, point, 10, trial);
                let mut rng = StdRng::seed_from_u64(seed);
                let payload: Vec<u8> = (0..40).map(|_| rng.random()).collect();
                let clean = frame.encode(&payload).unwrap();
                let fading = (channel == "poor").then(|| WattersonPreset::Poor.config());
                let mut cable = PersistentChannel::new(
                    48_000.0,
                    fading.as_ref(),
                    Some(SnrProfile::constant(snr as f32)),
                    coppa_channel::mean_power(&clean),
                    seed,
                    0,
                );
                // Unknown alignment and channel-generated noise on both sides.
                // One-second tail covers the next streaming decode cadence and
                // the fading channel's FIR delay, within the ten-second cap.
                let lead = 16_800 + usize::from(rng.random::<u16>()) % 2400;
                let mut capture = cable.process(&vec![0.0; lead]);
                capture.extend(cable.process(&clean));
                capture.extend(cable.process(&vec![0.0; 48_000]));
                let outcome = match frame.receive(&capture) {
                    Ok(got) if got == payload => AttemptOutcome::Delivered,
                    Ok(_) => AttemptOutcome::WrongPayload,
                    Err(FloorError::Payload) => AttemptOutcome::PayloadFailed,
                    Err(FloorError::Header) => AttemptOutcome::SyncFailed(SyncCause::HeaderDecode),
                    Err(FloorError::Acquisition) => {
                        AttemptOutcome::SyncFailed(SyncCause::NoDetection)
                    }
                    Err(FloorError::Capacity) => panic!("receive returned transmit capacity error"),
                };
                batch.record(outcome, FRAME_SAMPLES, payload.len());
                let mut rx = FloorStream::default();
                let mut delivered = Vec::new();
                for chunk in capture.chunks(4800) {
                    delivered.extend(rx.push(chunk));
                }
                // The streaming API exposes no trusted failure-stage trace.
                let outcome = if delivered.iter().any(|got| got != &payload) {
                    AttemptOutcome::WrongPayload
                } else if delivered.len() == 1 {
                    AttemptOutcome::Delivered
                } else {
                    AttemptOutcome::SyncFailed(SyncCause::NoDetection)
                };
                streaming.record(outcome, FRAME_SAMPLES, payload.len());
            }
            eprintln!(
                "{channel} {snr} dB: batch={}, streaming={} errors",
                batch.frame_errors(),
                streaming.frame_errors()
            );
            batch_points.push(batch);
            stream_points.push(streaming);
        }
        let mut csv = format!("{CSV_HEADER}\n");
        for (rx, points) in [("batch", &batch_points), ("streaming", &stream_points)] {
            for point in points {
                csv.push_str(&point.csv_row("floor-v2", rx, channel));
                csv.push('\n');
            }
        }
        let bar = if channel == "poor" { -2.0 } else { -8.0 };
        let batch_verdict = evaluate(&batch_points, bar);
        let stream_verdict = evaluate(&stream_points, bar);
        let metadata = format!(
            "waveform=floor-v2\nproduction=true\nacceptance_run=false\nair_protocol_version=2\nseed_base={DEV_SEED_BASE}\nseed_derivation=attempt_seed(base,point,10,trial); persistent direction=0\ntrials_per_point=10\nchannel={channel}\nframe_samples={FRAME_SAMPLES}\npayload_bytes=40\nsample_rate=48000\ncfo_hz=0\nssb=false\nrx=FloorFrame::receive,FloorStream::push\nlead_samples=16800..19199\ntail_samples=48000\nstream_chunk_samples=4800\nstream_failure_accounting=no-delivery-is-sync-no-detection; no trusted stage trace\nbatch_verdict={batch_verdict:?}\nstreaming_verdict={stream_verdict:?}\nwall_seconds={}\ncommand=cargo test --release -p coppa-bench --test floor_results record_production_floor_development_sweeps -- --ignored --nocapture\n",
            started.elapsed().as_secs_f64()
        );
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../results/cop-190")
            .join(format!("dev-{channel}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("sweep.csv"), csv).unwrap();
        fs::write(root.join("metadata.txt"), metadata).unwrap();
    }
}
