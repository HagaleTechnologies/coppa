//! COP-170: audits the committed comparison data and page. The page must be
//! exactly what the committed CSV renders, every published row must be a
//! coppa row, and every trial must carry the seed the grid gives it.

use std::fs;
use std::path::{Path, PathBuf};

use coppa_bench::compare::grid::{cells, Channel, SEED_BASE, SIZES, SNRS_DB};
use coppa_bench::compare::link_stack::row_spec;
use coppa_bench::compare::render::{
    anchor, data_regions, first_difference, recorded_version, regions, render_page,
};
use coppa_bench::compare::trial::{read_csv, Outcome, HEADER};
use coppa_bench::compare::{default_out_dir, default_page, repo_root};

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn published_csv() -> PathBuf {
    default_out_dir().join("link-l2.csv")
}

fn expected_page() -> (String, String) {
    let page = read(&default_page());
    let trials = read_csv(&published_csv()).unwrap();
    let metadata = fs::read_to_string(default_out_dir().join("metadata.md")).ok();
    let version = recorded_version(metadata.as_deref());
    let rows = [row_spec("link-l2").unwrap()];
    let mut bodies = regions(&rows, &trials, &version);
    bodies.extend(data_regions(&default_out_dir()));
    let expected = render_page(&page, &bodies).unwrap();
    (page, expected)
}

#[test]
fn published_page_matches_committed_trials() {
    let (page, expected) = expected_page();
    assert_eq!(
        first_difference(&page, &expected),
        None,
        "docs/benchmarks/comparison.md is stale; run cargo run --release -p coppa-bench -- compare"
    );
}

#[test]
fn rendering_the_committed_data_is_a_no_op() {
    let (page, expected) = expected_page();
    let trials = read_csv(&published_csv()).unwrap();
    let rows = [row_spec("link-l2").unwrap()];
    let metadata = fs::read_to_string(default_out_dir().join("metadata.md")).ok();
    let version = recorded_version(metadata.as_deref());
    let mut bodies = regions(&rows, &trials, &version);
    bodies.extend(data_regions(&default_out_dir()));
    let again = render_page(&expected, &bodies).unwrap();
    assert_eq!(again, expected);
    assert_eq!(page, expected);
}

#[test]
fn published_trials_follow_the_grid() {
    let text = read(&published_csv());
    assert!(text.starts_with(&format!("{HEADER}\n")));
    let trials = read_csv(&published_csv()).unwrap();
    assert!(!trials.is_empty());
    let grid = cells(&Channel::ALL, &SNRS_DB, &SIZES);
    for t in &trials {
        let cell = grid
            .iter()
            .find(|c| {
                c.channel == t.channel && c.snr_db == t.snr_db && c.size_bytes == t.size_bytes
            })
            .unwrap_or_else(|| panic!("trial off the grid: {}", t.to_line()));
        assert_eq!(
            Some(t.seed),
            cell.seed(SEED_BASE, t.trial),
            "{}",
            t.to_line()
        );
        assert!(t.exact, "inexact delivery: {}", t.to_line());
        assert!(t.trial < 8, "{}", t.to_line());
    }
    // Every one of the 42 cells (3 channels x 7 SNRs x 2 sizes) holds exactly
    // trials 0-7 (COP-215 recorded the last 16 KB cells), so the bare
    // `compare` has no trial left to run.
    for cell in &grid {
        let mut idx: Vec<usize> = trials
            .iter()
            .filter(|t| {
                t.channel == cell.channel
                    && t.snr_db == cell.snr_db
                    && t.size_bytes == cell.size_bytes
            })
            .map(|t| t.trial)
            .collect();
        idx.sort_unstable();
        assert_eq!(idx, (0..8).collect::<Vec<_>>(), "{cell:?}");
    }
    assert_eq!(grid.len(), SNRS_DB.len() * Channel::ALL.len() * SIZES.len());
    assert_eq!(
        trials.len(),
        SNRS_DB.len() * Channel::ALL.len() * SIZES.len() * 8
    );
    // The file is sorted, so its bytes do not depend on --jobs.
    let mut sorted = trials.clone();
    coppa_bench::compare::runner::sort_rows(&mut sorted);
    assert_eq!(sorted, trials);
    assert!(trials.iter().any(|t| t.outcome == Outcome::Complete));
}

/// AC6: `compare --check` exits 0 on the committed tree. COP-215 AC3: it
/// renders all 336 trials of the 42-cell grid.
#[test]
fn check_passes_on_the_committed_tree() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_coppa-bench"))
        .args(["compare", "--check"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "compare --check failed ({:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("matches"), "{stderr}");
    let all = SNRS_DB.len() * Channel::ALL.len() * SIZES.len() * 8;
    assert!(stderr.contains(&format!("({all} trials)")), "{stderr}");
}

/// The body of the page's generated region `name`.
fn region<'a>(page: &'a str, name: &str) -> &'a str {
    let begin = format!("<!-- compare:begin {name} -->");
    let end = format!("<!-- compare:end {name} -->");
    let start = page
        .find(&begin)
        .unwrap_or_else(|| panic!("comparison.md has no {begin}"))
        + begin.len();
    let stop = page[start..]
        .find(&end)
        .unwrap_or_else(|| panic!("comparison.md has no {end}"))
        + start;
    &page[start..stop]
}

/// COP-215 AC3: the published page has a value in all 42 cells. No table
/// cell is the dash an unrecorded cell renders as, every cell's value ends
/// in `k/8` and links to its cell anchor, and the replay region holds that
/// anchor.
#[test]
fn published_page_has_a_value_in_every_cell() {
    let page = read(&default_page());
    let tables = region(&page, "tables");
    let replay = region(&page, "replay");
    let dashed: Vec<_> = tables.lines().filter(|l| l.contains("| - ")).collect();
    assert!(
        dashed.is_empty(),
        "{} table rows show an unrecorded cell: {dashed:?}",
        dashed.len()
    );
    for cell in cells(&Channel::ALL, &SNRS_DB, &SIZES) {
        let id = anchor("link-l2", cell.channel, cell.snr_db, cell.size_bytes);
        assert!(
            tables.contains(&format!("/8](#{id})")),
            "no 8-trial value links to #{id}"
        );
        assert!(
            replay.contains(&format!("<a id=\"{id}\"></a>")),
            "no replay row for #{id}"
        );
    }
}

#[test]
fn published_data_is_coppa_only() {
    for entry in fs::read_dir(default_out_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "csv") {
            if path
                .file_name()
                .is_some_and(|name| name == "channel-check.csv")
            {
                assert!(read(&path).starts_with("preset,sample_rate,seed,"));
                continue;
            }
            if path
                .file_name()
                .is_some_and(|name| name == "blackbox-coppad.csv")
            {
                let text = read(&path);
                assert!(text.starts_with("modem,adapter,version_reply,"));
                for row in text.lines().skip(1) {
                    assert!(
                        row.starts_with("coppad,"),
                        "{}: foreign row",
                        path.display()
                    );
                    assert!(
                        row.contains("VERSION Coppa"),
                        "{}: foreign version",
                        path.display()
                    );
                }
                continue;
            }
            assert_eq!(
                path.file_name().unwrap(),
                "link-l2.csv",
                "unknown published CSV"
            );
            for t in read_csv(&path).unwrap() {
                assert!(
                    t.row.starts_with("link-") && row_spec(&t.row).is_some(),
                    "{}: non-coppa row {}",
                    path.display(),
                    t.row
                );
            }
        }
    }
    let page = read(&default_page());
    for line in page
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| Row"))
    {
        let first = line.split('|').nth(1).unwrap_or_default().trim();
        assert!(
            first.starts_with("coppa ")
                || first.starts_with("<a id=\"cell-link-")
                || first.starts_with("Good (")
                || first == "Moderate"
                || first.starts_with("Poor (")
                || first == "Preset"
                || first.starts_with("coppad ")
                || first == "Modem"
                || first == "Cell",
            "non-coppa table row: {line}"
        );
    }
}

#[test]
fn page_states_documented_offsets() {
    let page = read(&default_page());
    for expected in [
        "IONOS",
        "README_ofdm.md",
        "PathSim",
        "1 ms",
        "2 ms",
        "not measured",
    ] {
        assert!(page.contains(expected), "missing {expected}");
    }
}

#[test]
fn ci_full_lints_the_blackbox_feature() {
    let workflow = read(&repo_root().join(".github/workflows/ci-full.yml"));
    assert!(workflow.contains(
        "cargo clippy -p coppa-bench --features cpal-backend --all-targets -- -D warnings"
    ));
}

#[test]
fn docs_name_both_compare_commands_and_audio_feature() {
    let readme = read(&repo_root().join("crates/coppa-bench/README.md"));
    for expected in [
        "compare channel-check",
        "compare blackbox",
        "--features cpal-backend",
        "f32le",
    ] {
        assert!(readme.contains(expected), "README lacks {expected}");
    }
    let changelog = read(&repo_root().join("CHANGELOG.md"));
    assert!(changelog.contains("compare blackbox"));
    assert!(changelog.contains("compare channel-check"));
}

#[test]
fn channel_cross_check_records_three_passing_presets() {
    let rows =
        coppa_bench::compare::channel_check::read_csv(&default_out_dir().join("channel-check.csv"))
            .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter().map(|r| r.preset).collect::<Vec<_>>(),
        ["good", "moderate", "poor"]
    );
    assert!(rows
        .iter()
        .all(|r| r.sample_rate == 48_000 && r.within_tolerance));
}

#[test]
fn blackbox_selftest_records_every_field() {
    let path = default_out_dir().join("blackbox-coppad.csv");
    let text = read(&path);
    let mut rows = text.lines();
    assert_eq!(rows.next(), Some("modem,adapter,version_reply,binary_sha256,mode,channel,snr_db,size_bytes,trial,seed,ref_power_a,ref_power_b,cap_s,outcome,connect_s,delivered_bytes,exact,transfer_s,net_bytes_per_min,drops,wall_s,cable_latency_ms,starved_ticks,overflow_ticks"));
    let lines: Vec<_> = rows.collect();
    assert_eq!(lines.len(), 6);
    for (channel, snr) in [(Channel::Wgn, 30), (Channel::Mpg, 20)] {
        let cell = coppa_bench::compare::grid::Cell {
            channel,
            snr_db: snr,
            size_bytes: 2048,
        };
        for trial in 0..3 {
            let fields: Vec<_> = lines
                .iter()
                .map(|l| l.split(',').collect::<Vec<_>>())
                .find(|f| {
                    f[5] == channel.name() && f[6] == snr.to_string() && f[8] == trial.to_string()
                })
                .unwrap();
            assert_eq!(fields.len(), 24);
            assert_eq!(fields[0], "coppad");
            assert!(fields[2].starts_with("VERSION Coppa"));
            assert_eq!(
                fields[9],
                format!("0x{:x}", cell.seed(SEED_BASE, trial).unwrap())
            );
            assert!(fields
                .iter()
                .enumerate()
                .all(|(i, value)| !value.is_empty() || i == 14 && fields[13] == "no_connect"));
        }
    }
}

#[test]
fn page_records_public_dialect_and_channel_sources() {
    let page = read(&default_page());
    for source in [
        "docs/HOST-API.md",
        "docs/ARDOP.md",
        "KISS TNC paper",
        "IONOS manual",
        "README_ofdm.md",
        "PathSim's page",
    ] {
        assert!(page.contains(source), "page lacks {source}");
    }
    assert!(page.contains("No other modem's source was read"));
    assert!(page.contains("no other modem was run for any number"));
}

#[test]
fn metadata_names_version_commands_and_sources() {
    let m = read(&default_out_dir().join("metadata.md"));
    for needle in [
        "coppa_version=",
        "seed_base=0x17000000",
        "trials_per_cell=8",
        "platform=linux-x86_64",
        "rustc=",
        "coppa-bench compare --channel wgn --size 2048",
        "coppa-bench compare channel-check",
        "coppa-bench compare blackbox --modem tools/compare/coppad.toml",
        "2020 Winlink",
    ] {
        assert!(m.contains(needle), "metadata.md lacks {needle:?}");
    }
}

#[test]
fn page_records_clean_room_provenance() {
    let page = read(&default_page());
    assert!(page.contains("## Provenance and clean-room statement"));
    assert!(page.contains("No other modem's source was read"));
    assert!(page.contains("no other modem was run for any number on this page"));
    assert!(page.contains("This page publishes coppa rows only"));
}

#[test]
fn docs_point_at_the_comparison_page() {
    let root = repo_root();
    let benchmarks = read(&root.join("BENCHMARKS.md"));
    assert!(benchmarks.contains("## COP-170 same-simulator comparison"));
    assert!(benchmarks.contains("(docs/benchmarks/comparison.md)"));
    let readme = read(&root.join("crates/coppa-bench/README.md"));
    assert!(readme.contains("-- compare"));
    let limitations = read(&root.join("docs/LIMITATIONS.md"));
    assert!(limitations.contains("## COP-170 same-simulator comparison"));
    let changelog = read(&root.join("CHANGELOG.md"));
    assert!(changelog.contains("coppa-bench compare"));
}

/// Re-runs four committed trials, one of them a fading 16 KB trial (COP-215),
/// and compares the rows byte for byte.
/// Run with `cargo test --release -p coppa-bench --test compare_results
/// committed_trials_reproduce -- --ignored`.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "re-runs committed trials; run by hand at publication"]
fn committed_trials_reproduce() {
    use coppa_bench::compare::link_stack::{
        reference_power, run_link_trial, TrialOptions, TrialSpec,
    };
    let row = row_spec("link-l2").unwrap();
    let p = reference_power(row.level).unwrap();
    let trials = read_csv(&published_csv()).unwrap();
    for (channel, snr, size) in [
        (Channel::Wgn, 0, 2048),
        (Channel::Wgn, 30, 2048),
        (Channel::Mpg, 20, 2048),
        (Channel::Mpp, 20, 16384),
    ] {
        let committed = trials
            .iter()
            .find(|t| {
                t.channel == channel && t.snr_db == snr && t.size_bytes == size && t.trial == 0
            })
            .unwrap_or_else(|| panic!("{channel:?} {snr} dB {size} B trial 0 is not committed"));
        let spec = TrialSpec {
            channel,
            snr_db: snr,
            size_bytes: size,
            trial: 0,
            seed: committed.seed,
        };
        let again = run_link_trial(&row, &spec, p, TrialOptions::default()).unwrap();
        assert_eq!(again.to_line(), committed.to_line());
    }
}
