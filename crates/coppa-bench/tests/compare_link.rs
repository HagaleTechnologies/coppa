//! COP-170: the in-process Link runner end to end (AC4, AC5). Trials that
//! must not connect use the hidden mute hook so they stay a few seconds long
//! in a debug build.

use std::fs;
use std::path::PathBuf;

use coppa_bench::compare::grid::Channel;
use coppa_bench::compare::link_stack::{
    reference_power, row_spec, run_link_trial, TrialOptions, TrialSpec,
};
use coppa_bench::compare::runner::{csv_path, run_plan, Plan};
use coppa_bench::compare::trial::{Outcome, HEADER};

fn spec(size_bytes: usize) -> TrialSpec {
    TrialSpec {
        channel: Channel::Wgn,
        snr_db: 30,
        size_bytes,
        trial: 0,
        seed: 0x1700_0000,
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("coppa-compare-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn muted() -> TrialOptions {
    TrialOptions {
        muted: true,
        ..TrialOptions::default()
    }
}

fn plan(out_dir: PathBuf, trials: usize, jobs: usize) -> Plan {
    Plan {
        rows: vec![row_spec("link-l2").unwrap()],
        channels: vec![Channel::Wgn],
        snrs: vec![0],
        sizes: vec![2048],
        trials,
        seed_base: 0x1700_0000,
        jobs,
        out_dir,
    }
}

#[test]
fn small_transfer_completes_exactly() {
    let row = row_spec("link-l2").unwrap();
    let p = reference_power(row.level).unwrap();
    let r = run_link_trial(&row, &spec(112), p, TrialOptions::default()).unwrap();
    assert_eq!(r.outcome, Outcome::Complete, "{}", r.to_line());
    assert!(r.exact);
    assert_eq!(r.delivered_bytes, 112);
    assert!(r.net_bytes_per_min > 0.0);
    assert!(r.connect_s.is_some_and(|c| c > 0.0 && c < 120.0));
    assert_eq!(r.drops, 0);
}

#[test]
fn cut_after_connect_is_a_drop() {
    let row = row_spec("link-l2").unwrap();
    let p = reference_power(row.level).unwrap();
    let options = TrialOptions {
        mute_after_connect: true,
        ..TrialOptions::default()
    };
    let r = run_link_trial(&row, &spec(2048), p, options).unwrap();
    assert_eq!(r.outcome, Outcome::Drop, "{}", r.to_line());
    assert_eq!(r.drops, 1);
    assert!(r.delivered_bytes < 2048);
    assert!(r.connect_s.is_some());
}

#[test]
fn muted_channel_never_connects() {
    let row = row_spec("link-l2").unwrap();
    let p = reference_power(row.level).unwrap();
    let r = run_link_trial(&row, &spec(2048), p, muted()).unwrap();
    assert_eq!(r.outcome, Outcome::NoConnect, "{}", r.to_line());
    assert_eq!(r.connect_s, None);
    assert_eq!(r.delivered_bytes, 0);
    assert_eq!(r.net_bytes_per_min, 0.0);
}

#[test]
fn jobs_do_not_change_the_csv() {
    let one = scratch("jobs1");
    let two = scratch("jobs2");
    let mut sink = Vec::new();
    run_plan(&plan(one.clone(), 2, 1), muted(), &mut sink).unwrap();
    run_plan(&plan(two.clone(), 2, 2), muted(), &mut sink).unwrap();
    let row = row_spec("link-l2").unwrap();
    let a = fs::read(csv_path(&one, &row)).unwrap();
    let b = fs::read(csv_path(&two, &row)).unwrap();
    assert_eq!(a, b);
    let text = String::from_utf8(a).unwrap();
    assert!(text.starts_with(HEADER));
    assert_eq!(text.lines().count(), 3);
    let _ = fs::remove_dir_all(one);
    let _ = fs::remove_dir_all(two);
}

#[test]
fn resume_runs_only_missing_trials() {
    let dir = scratch("resume");
    let row = row_spec("link-l2").unwrap();
    let path = csv_path(&dir, &row);
    let mut sink = Vec::new();
    let first = run_plan(&plan(dir.clone(), 3, 1), muted(), &mut sink).unwrap();
    assert_eq!(first[0].ran.len(), 3);
    let complete = fs::read_to_string(&path).unwrap();

    // Delete trial 0's line and cut trial 2's (the last) mid-line: the torn
    // line is dropped, and only trials 0 and 2 re-run.
    let lines: Vec<&str> = complete.lines().collect();
    assert_eq!(lines.len(), 4, "header and three trials");
    let torn = &lines[3][..lines[3].len() - 10];
    fs::write(&path, format!("{}\n{}\n{torn}", lines[0], lines[2])).unwrap();
    let mut log = Vec::new();
    let again = run_plan(&plan(dir.clone(), 3, 1), muted(), &mut log).unwrap();
    assert_eq!(again[0].recorded, 1);
    let mut reran: Vec<usize> = again[0].ran.iter().map(|key| key.4).collect();
    reran.sort_unstable();
    assert_eq!(reran, vec![0, 2], "only the missing trials re-run");
    let log = String::from_utf8(log).unwrap();
    assert!(
        log.contains(" trial 0 seed ") && log.contains(" trial 2 seed "),
        "{log}"
    );
    assert!(!log.contains(" trial 1 seed "), "{log}");
    assert_eq!(fs::read_to_string(&path).unwrap(), complete);

    // A complete file runs nothing and keeps its bytes.
    let idle = run_plan(&plan(dir.clone(), 3, 1), muted(), &mut sink).unwrap();
    assert!(idle[0].ran.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), complete);

    // A recorded seed that the plan does not give is refused.
    let mut other = plan(dir.clone(), 3, 1);
    other.seed_base = 0x1800_0000;
    let err = run_plan(&other, muted(), &mut sink).unwrap_err();
    assert!(format!("{err:#}").contains("seed"), "{err:#}");
    let _ = fs::remove_dir_all(dir);
}
