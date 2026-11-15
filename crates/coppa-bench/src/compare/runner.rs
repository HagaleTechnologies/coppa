//! COP-170: runs the plan's pending trials on worker threads, appends each
//! finished row to the row's CSV, and on completion rewrites the file sorted
//! so its bytes do not depend on `--jobs` or on resumes.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use anyhow::{bail, Context, Result};

use super::grid::{cells, Channel};
use super::link_stack::{reference_power, run_link_trial, RowSpec, TrialOptions, TrialSpec};
use super::trial::{read_csv, to_csv, TrialKey, TrialRow, HEADER};

/// What to measure.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Rows.
    pub rows: Vec<RowSpec>,
    /// Channels.
    pub channels: Vec<Channel>,
    /// SNR points (dB).
    pub snrs: Vec<i32>,
    /// Sizes (bytes).
    pub sizes: Vec<usize>,
    /// Trials per cell.
    pub trials: usize,
    /// Seed of trial 0 of cell 0.
    pub seed_base: u64,
    /// Worker threads.
    pub jobs: usize,
    /// One CSV per row lives here.
    pub out_dir: PathBuf,
}

/// The CSV of `row` in `out_dir`.
#[must_use]
pub fn csv_path(out_dir: &Path, row: &RowSpec) -> PathBuf {
    out_dir.join(format!("{}.csv", row.name))
}

/// Sort order of a results file: row, channel, SNR (high first), size, trial.
pub fn sort_rows(rows: &mut [TrialRow]) {
    rows.sort_by(|a, b| {
        (&a.row, a.channel, -a.snr_db, a.size_bytes, a.trial).cmp(&(
            &b.row,
            b.channel,
            -b.snr_db,
            b.size_bytes,
            b.trial,
        ))
    });
}

/// One pending trial.
#[derive(Debug, Clone, Copy)]
struct Job {
    row: RowSpec,
    spec: TrialSpec,
    ref_power: f32,
}

/// What a run did, per row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    /// Trials the plan names.
    pub planned: usize,
    /// Trials already recorded.
    pub recorded: usize,
    /// Trials run now, in completion order of their keys.
    pub ran: Vec<TrialKey>,
}

/// The trials of `plan` for `row`, keyed, with their seeds.
fn plan_specs(plan: &Plan, row: &RowSpec) -> Result<Vec<(TrialKey, TrialSpec)>> {
    let mut out = Vec::new();
    for cell in cells(&plan.channels, &plan.snrs, &plan.sizes) {
        for trial in 0..plan.trials {
            let seed = cell
                .seed(plan.seed_base, trial)
                .with_context(|| format!("trial {trial} is beyond the seed stride"))?;
            out.push((
                (
                    row.name.to_string(),
                    cell.channel,
                    cell.snr_db,
                    cell.size_bytes,
                    trial,
                ),
                TrialSpec {
                    channel: cell.channel,
                    snr_db: cell.snr_db,
                    size_bytes: cell.size_bytes,
                    trial,
                    seed,
                },
            ));
        }
    }
    Ok(out)
}

/// Rough cost order: 16 KB before 2 KB, fading before WGN, high SNR first.
fn cost_rank(spec: &TrialSpec) -> (std::cmp::Reverse<usize>, std::cmp::Reverse<Channel>) {
    (
        std::cmp::Reverse(spec.size_bytes),
        std::cmp::Reverse(spec.channel),
    )
}

/// Run every trial of `plan` that is not yet recorded. Progress goes to
/// `log`. Test hooks in `options` apply to every trial.
pub fn run_plan(
    plan: &Plan,
    options: TrialOptions,
    log: &mut dyn Write,
) -> Result<Vec<RunSummary>> {
    fs::create_dir_all(&plan.out_dir)
        .with_context(|| format!("creating {}", plan.out_dir.display()))?;
    let mut summaries = Vec::new();
    for row in &plan.rows {
        let path = csv_path(&plan.out_dir, row);
        let recorded = read_csv(&path)?;
        let by_key: HashMap<TrialKey, &TrialRow> = recorded.iter().map(|r| (r.key(), r)).collect();
        let specs = plan_specs(plan, row)?;
        let ref_power = reference_power(row.level)?;
        let mut jobs = Vec::new();
        let mut already = 0;
        for (key, spec) in &specs {
            match by_key.get(key) {
                Some(r) if r.seed != spec.seed => bail!(
                    "{}: trial {:?} was recorded with seed 0x{:08x}, the plan gives 0x{:08x}; use another --out-dir or --seed-base",
                    path.display(),
                    key,
                    r.seed,
                    spec.seed
                ),
                Some(_) => already += 1,
                None => jobs.push(Job {
                    row: *row,
                    spec: *spec,
                    ref_power,
                }),
            }
        }
        writeln!(
            log,
            "compare: plan {} x {} trials; {} recorded in {}; {} to run; jobs {}",
            row.name,
            specs.len(),
            already,
            path.display(),
            jobs.len(),
            plan.jobs.max(1)
        )?;
        jobs.sort_by_key(|j| cost_rank(&j.spec));

        // Rewrite the recorded rows first: drops a torn last line and makes
        // sure the header is present before appends.
        let mut kept = recorded.clone();
        sort_rows(&mut kept);
        write_atomic(&path, &to_csv(&kept))?;

        let ran = run_jobs(&jobs, plan.jobs.max(1), options, &path, log)?;

        let mut all = read_csv(&path)?;
        sort_rows(&mut all);
        all.dedup_by(|a, b| a.key() == b.key());
        write_atomic(&path, &to_csv(&all))?;
        summaries.push(RunSummary {
            planned: specs.len(),
            recorded: already,
            ran,
        });
    }
    Ok(summaries)
}

fn run_jobs(
    jobs: &[Job],
    workers: usize,
    options: TrialOptions,
    path: &Path,
    log: &mut dyn Write,
) -> Result<Vec<TrialKey>> {
    let mut ran = Vec::new();
    if jobs.is_empty() {
        return Ok(ran);
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let next = AtomicUsize::new(0);
    let total = jobs.len();
    let (tx, rx) = mpsc::channel::<Result<(TrialRow, f64)>>();
    std::thread::scope(|scope| -> Result<()> {
        for _ in 0..workers.min(total) {
            let tx = tx.clone();
            let next = &next;
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(job) = jobs.get(i) else { break };
                let started = wall_clock();
                let result = run_link_trial(&job.row, &job.spec, job.ref_power, options)
                    .map(|row| (row, started.elapsed().as_secs_f64()));
                if tx.send(result).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        let mut first_error = None;
        for (done, result) in rx.iter().enumerate() {
            match result {
                Ok((row, wall)) => {
                    let mut line = row.to_line();
                    line.push('\n');
                    file.write_all(line.as_bytes())?;
                    file.flush()?;
                    writeln!(
                        log,
                        "[{:>3}/{}] {} {} {} dB {} B trial {} seed 0x{:08x}: {} ({:.1} s wall)",
                        done + 1,
                        total,
                        row.row,
                        row.channel.name(),
                        row.snr_db,
                        row.size_bytes,
                        row.trial,
                        row.seed,
                        describe(&row),
                        wall
                    )?;
                    ran.push(row.key());
                }
                Err(e) => {
                    // Stop handing out work; finish what is running.
                    next.store(usize::MAX / 2, Ordering::SeqCst);
                    first_error.get_or_insert(e);
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    })?;
    Ok(ran)
}

/// The progress-line summary of a row.
#[must_use]
pub fn describe(row: &TrialRow) -> String {
    match row.connect_s {
        None => row.outcome.name().to_string(),
        Some(c) => format!(
            "{}, connect {:.2} s, {} B, {:.1} B/min",
            row.outcome.name(),
            c,
            row.delivered_bytes,
            row.net_bytes_per_min
        ),
    }
}

// Wall time is only printed, never recorded, so the CSV stays reproducible.
#[allow(clippy::disallowed_methods)]
fn wall_clock() -> std::time::Instant {
    std::time::Instant::now()
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("csv.tmp");
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming onto {}", path.display()))?;
    debug_assert!(text.starts_with(HEADER));
    Ok(())
}
