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
    bodies.extend(data_regions(&default_out_dir()).unwrap());
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
    bodies.extend(data_regions(&default_out_dir()).unwrap());
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
                // Decision 10: the modem column is coppad and the version
                // reply column itself starts with "VERSION Coppa".
                for row in text.lines().skip(1) {
                    let fields: Vec<_> = row.split(',').collect();
                    assert_eq!(
                        fields.len(),
                        coppa_bench::compare::blackbox::CSV_COLUMNS,
                        "{}: malformed row {row}",
                        path.display()
                    );
                    assert_eq!(fields[0], "coppad", "{}: foreign row {row}", path.display());
                    assert!(
                        fields
                            .get(2)
                            .is_some_and(|v| v.starts_with("VERSION Coppa")),
                        "{}: foreign version reply in {row}",
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
    assert_eq!(rows.next(), Some("modem,adapter,version_reply,binary_sha256,mode,channel,snr_db,size_bytes,trial,seed,ref_power_a,ref_power_b,cap_s,outcome,connect_s,delivered_bytes,exact,transfer_s,net_bytes_per_min,drops,wall_s,cable_latency_ms,starved_ticks,overflow_ticks,payload_crc32"));
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
            assert_eq!(fields.len(), 25);
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
            // COP-227 (AC9): the published row follows the delivered_bytes rule.
            let size: usize = fields[7].parse().unwrap();
            let delivered: usize = fields[15].parse().unwrap();
            let exact: bool = fields[16].parse().unwrap();
            let transfer_s: f64 = fields[17].parse().unwrap();
            let rate: f64 = fields[18].parse().unwrap();
            assert!(
                delivered <= size,
                "delivered_bytes above size_bytes: {fields:?}"
            );
            assert_eq!(
                fields[13] == "complete",
                exact,
                "complete must mean exact: {fields:?}"
            );
            assert!(
                !exact || delivered == size,
                "an exact row delivered less than its payload: {fields:?}"
            );
            let expected = if transfer_s > 0.0 {
                delivered as f64 * 60.0 / transfer_s
            } else {
                0.0
            };
            assert!(
                (rate - expected).abs() <= 0.01 + 1e-4 * expected,
                "net_bytes_per_min {rate} is not delivered_bytes per minute {expected}: {fields:?}"
            );
        }
    }
}

/// COP-227 (AC1): every published row carries the seeded payload for its own
/// seed, recorded by one coppad binary that is not the ramp-era one. The CRC
/// is computed here with the `crc` crate, not through the harness.
#[test]
fn blackbox_selftest_rows_carry_the_seeded_payload() {
    use crc::{Crc, CRC_32_ISO_HDLC};
    let text = read(&default_out_dir().join("blackbox-coppad.csv"));
    let mut lines = text.lines();
    let header: Vec<_> = lines.next().unwrap().split(',').collect();
    assert_eq!(header.last(), Some(&"payload_crc32"));
    let column = |name: &str| {
        header
            .iter()
            .position(|h| *h == name)
            .unwrap_or_else(|| panic!("no {name} column"))
    };
    let (sha, seed, size, crc) = (
        column("binary_sha256"),
        column("seed"),
        column("size_bytes"),
        column("payload_crc32"),
    );
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
    assert_eq!(rows.len(), 6);
    let first_sha = rows[0][sha];
    assert_ne!(
        first_sha, "9b8b4957d0c18b1df65c23d26a97b2a855101fff94fb6902757ff0c1d6d1327e",
        "a ramp-era row is still published"
    );
    for row in &rows {
        assert_eq!(row.len(), header.len(), "{row:?}");
        assert_eq!(row[sha], first_sha, "rows from two binaries: {row:?}");
        let seed = u64::from_str_radix(row[seed].strip_prefix("0x").unwrap(), 16).unwrap();
        let size: usize = row[size].parse().unwrap();
        let payload = coppa_bench::compare::link_stack::payload(seed, size);
        let expected = format!(
            "0x{:08x}",
            Crc::<u32>::new(&CRC_32_ISO_HDLC).checksum(&payload)
        );
        assert_eq!(row[crc], expected, "seed 0x{seed:x}: {row:?}");
    }
}

/// COP-227 (AC10): the page states what a trial writes and how
/// `delivered_bytes` is scored.
#[test]
fn blackbox_section_states_the_scoring_rule() {
    let page = read(&default_page());
    let text = section(&page, "## Black-box self-test")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for needle in [
        "same seeded bytes",
        "`payload_crc32`",
        "before the first byte that differs from the payload",
        "do not count",
        "equal the payload exactly",
        "counts once",
    ] {
        assert!(
            text.contains(needle),
            "Black-box self-test lacks {needle:?}"
        );
    }
}

/// COP-227 (AC2): `metadata.md` records the re-recorded rows as the CSV has
/// them, and an incomplete re-record is stated in LIMITATIONS.md.
#[test]
fn metadata_matches_the_blackbox_rows() {
    let metadata = read(&default_out_dir().join("metadata.md"));
    let value = |key: &str| -> String {
        let prefix = format!("{key}=");
        metadata
            .lines()
            .find_map(|line| line.trim().strip_prefix(&prefix).map(str::to_string))
            .unwrap_or_else(|| panic!("metadata.md lacks {prefix}"))
    };
    let text = read(&default_out_dir().join("blackbox-coppad.csv"));
    let rows: Vec<Vec<&str>> = text
        .lines()
        .skip(1)
        .map(|l| l.split(',').collect())
        .collect();
    let version = value("blackbox_version_reply");
    let sha = value("blackbox_binary_sha256");
    for row in &rows {
        assert_eq!(row[2], version, "{row:?}");
        assert_eq!(row[3], sha, "{row:?}");
    }
    assert_eq!(value("blackbox_payload"), "seeded");
    let count = rows.len();
    let completed = rows.iter().filter(|r| r[13] == "complete").count();
    assert_eq!(value("blackbox_rows"), count.to_string());
    assert_eq!(value("blackbox_completed"), completed.to_string());
    let joined = metadata.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        !joined.contains("predate the payload fix"),
        "metadata.md still describes the ramp-era rows as published"
    );
    if completed < count {
        assert!(
            !joined.contains("All six trial rows completed exactly"),
            "metadata.md says every row completed"
        );
        let limitations = read(&repo_root().join("docs/LIMITATIONS.md"));
        let comparison = section(&limitations, "## COP-170 same-simulator comparison")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for needle in [
            "COP-227".to_string(),
            "COP-237".to_string(),
            format!("{completed}/{count} completed"),
        ] {
            assert!(
                comparison.contains(&needle),
                "LIMITATIONS.md lacks {needle}"
            );
        }
    }
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

/// COP-226: the payload-fix note reports the measured frame outcome.
#[test]
fn ramp_note_states_the_measured_frame_outcome() {
    let m = read(&default_out_dir().join("metadata.md"));
    // Join wrapped lines so a phrase split across a line break still counts.
    let m = m.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(!m.contains("would have overstated"));
    for needle in [
        "no frame holds more than 1424",
        "matched in every case",
        "re-record all six",
    ] {
        assert!(m.contains(needle), "metadata.md lacks {needle:?}");
    }
}

/// COP-226: no coppad frame holds enough raw ramp bytes to compress better
/// than seeded bytes.
#[test]
fn no_coppad_frame_holds_enough_ramp_to_compress() {
    use coppa_engine::{profiles::HF_STANDARD, CoppaCore, EngineConfig, HF_MAX_SPEED_LEVEL};
    use coppa_protocol::modem::{
        speed_levels::max_multi_payload_for_level, transceiver::MAX_CODEWORDS,
    };
    let largest = (1..=HF_MAX_SPEED_LEVEL)
        .filter_map(|l| max_multi_payload_for_level(l, MAX_CODEWORDS))
        .max()
        .unwrap();
    assert_eq!(
        largest, 1424,
        "update results/compare/metadata.md if this changes"
    );
    let core = CoppaCore::with_config(EngineConfig::from_profile(&HF_STANDARD));
    // The six recorded trials' seeds. Before PR #281's payload fix, the
    // harness wrote `(seed + i) & 0xff` for each.
    let csv = read(&default_out_dir().join("blackbox-coppad.csv"));
    let mut rows = csv.lines();
    let column = rows
        .next()
        .and_then(|h| h.split(',').position(|name| name == "seed"))
        .unwrap();
    let seeds: Vec<u64> = rows
        .map(|row| {
            let hex = row.split(',').nth(column).unwrap();
            u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap()
        })
        .collect();
    assert_eq!(seeds.len(), 6);
    for seed in seeds {
        let ramp: Vec<u8> = (0..2048u64)
            .map(|i| (seed.wrapping_add(i) & 0xff) as u8)
            .collect();
        let seeded = coppa_bench::compare::link_stack::payload(seed, 2048);
        assert!(core.encoded_payload_len(&ramp) > largest);
        for n in [512, 1024, 2048] {
            let (r, s) = (
                core.encoded_payload_len(&ramp[..n]),
                core.encoded_payload_len(&seeded[..n]),
            );
            assert!(
                r * 100 >= s * 98,
                "seed {seed:#x}, {n} B: ramp {r}, seeded {s}"
            );
        }
    }
}

/// COP-226: the black-box section names its payload and the stack differences.
#[test]
fn blackbox_section_states_payload_and_compression() {
    let page = read(&default_page());
    let blackbox = section(&page, "## Black-box self-test");
    // Join wrapped lines so a phrase split across a line break still counts.
    let blackbox = blackbox.split_whitespace().collect::<Vec<_>>().join(" ");
    for needle in [
        "same seeded bytes",
        "256-byte repeating ramp",
        "do not compress",
    ] {
        assert!(
            blackbox.contains(needle),
            "black-box section lacks {needle:?}"
        );
    }
}

#[test]
fn page_records_clean_room_provenance() {
    let page = read(&default_page());
    assert!(page.contains("## Provenance and clean-room statement"));
    assert!(page.contains("No other modem's source was read"));
    assert!(page.contains("no other modem was run for any number on this page"));
    assert!(page.contains("This page publishes coppa rows only"));
    // COP-214 (AC11): the provenance section names the public source of each
    // host dialect and channel model.
    let provenance = section(&page, "## Provenance and clean-room statement");
    for source in [
        "docs/HOST-API.md",
        "docs/ARDOP.md",
        "KISS TNC paper",
        "IONOS manual",
        "README_ofdm.md",
        "PathSim's page",
    ] {
        assert!(provenance.contains(source), "provenance lacks {source}");
    }
}

/// The text of one `## ` section of a Markdown page, heading included. The
/// heading must start a line, so `### Status` or prose quoting a heading
/// cannot stand in for it.
fn section<'a>(page: &'a str, heading: &str) -> &'a str {
    let start = if page.starts_with(heading) {
        0
    } else {
        page.find(&format!("\n{heading}"))
            .unwrap_or_else(|| panic!("page lacks {heading}"))
            + 1
    };
    let rest = &page[start..];
    let end = rest[heading.len()..]
        .find("\n## ")
        .map_or(rest.len(), |i| heading.len() + i);
    &rest[..end]
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
    // COP-227 (AC12): the payload column is documented where the harness is.
    assert!(readme.contains("payload_crc32"));
    assert!(changelog.contains("payload_crc32"));

    // COP-214 (AC10): every document says what shipped.
    for (name, text) in [
        ("BENCHMARKS.md", &benchmarks),
        ("crates/coppa-bench/README.md", &readme),
        ("CHANGELOG.md", &changelog),
    ] {
        for command in ["compare blackbox", "compare channel-check"] {
            assert!(text.contains(command), "{name} lacks {command}");
        }
    }
    for expected in ["--features cpal-backend", "f32le", "release `coppad`"] {
        assert!(
            readme.contains(expected),
            "crates/coppa-bench/README.md lacks {expected}"
        );
    }
    let comparison = section(&limitations, "## COP-170 same-simulator comparison");
    assert!(
        !comparison.contains("Still open from the COP-170 plan"),
        "LIMITATIONS.md still lists the harness and cross-check as open"
    );
    // The f32-only cable and the planning probe's numbers.
    for expected in [
        "f32le",
        "48 kHz, mono",
        "0.772",
        "correlation 1.000000",
        "PathSim's delay offset is not measured",
    ] {
        assert!(
            comparison.contains(expected),
            "LIMITATIONS.md lacks {expected}"
        );
    }
    let page = read(&default_page());
    let status = section(&page, "## Status");
    assert!(status.contains("(COP-214)"), "page Status: {status}");
    assert!(!status.contains("Still to come"), "page Status: {status}");
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
