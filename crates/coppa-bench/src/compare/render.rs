//! COP-170: renders the per-cell statistics as Markdown tables, one per
//! channel and transfer size, with the SNR grid as columns. Each cell reads
//! "mean (95% interval) completed/n".

use std::fmt::Write as _;

use super::grid::{Cell, Channel, CELL_SEED_STRIDE, SIZES, SNRS_DB};
use super::link_stack::RowSpec;
use super::stats::{completion, t_interval};
use super::trial::{Outcome, TrialRow};

fn size_label(size: usize) -> String {
    if size % 1024 == 0 {
        format!("{} KB", size / 1024)
    } else {
        format!("{size} B")
    }
}

/// One cell's text, or "-" when no trial of the cell is recorded.
#[must_use]
pub fn cell_text(trials: &[&TrialRow]) -> String {
    let rates: Vec<f64> = trials.iter().map(|t| t.net_bytes_per_min).collect();
    let Some(ci) = t_interval(&rates) else {
        return "-".to_string();
    };
    let done = trials
        .iter()
        .filter(|t| t.outcome == Outcome::Complete)
        .count();
    let c = completion(done, trials.len());
    let hi = if ci.hi.is_finite() {
        format!("{:.0}", ci.hi)
    } else {
        "inf".to_string()
    };
    format!(
        "{:.0} ({:.0}-{}) {}/{}",
        ci.mean, ci.lo, hi, c.completed, c.n
    )
}

/// Anchor id of a cell's replay row.
#[must_use]
pub fn anchor(row: &str, channel: Channel, snr_db: i32, size_bytes: usize) -> String {
    format!("cell-{row}-{}-{snr_db}-{size_bytes}", channel.name())
}

fn cell_trials<'a>(
    trials: &'a [TrialRow],
    row: &str,
    channel: Channel,
    snr_db: i32,
    size_bytes: usize,
) -> Vec<&'a TrialRow> {
    trials
        .iter()
        .filter(|t| {
            t.row == row && t.channel == channel && t.snr_db == snr_db && t.size_bytes == size_bytes
        })
        .collect()
}

/// The replay table: one row per measured cell, with its anchor, counts,
/// seed range and the exact command that re-runs it into a scratch dir.
#[must_use]
pub fn render_replay(rows: &[RowSpec], trials: &[TrialRow]) -> String {
    let mut out = String::from(
        "| Cell | n | completed (Wilson 95%) | drops | no connect | median connect | seeds | replay |\n|---|---|---|---|---|---|---|---|\n",
    );
    for row in rows {
        for channel in Channel::ALL {
            for snr in SNRS_DB {
                for size in SIZES {
                    let cell = cell_trials(trials, row.name, channel, snr, size);
                    if cell.is_empty() {
                        continue;
                    }
                    let n = cell.len();
                    let done = cell
                        .iter()
                        .filter(|t| t.outcome == Outcome::Complete)
                        .count();
                    let c = completion(done, n);
                    let drops: u32 = cell.iter().map(|t| t.drops).sum();
                    let no_connect = cell
                        .iter()
                        .filter(|t| t.outcome == Outcome::NoConnect)
                        .count();
                    let mut connects: Vec<f64> = cell.iter().filter_map(|t| t.connect_s).collect();
                    connects.sort_by(f64::total_cmp);
                    let median = match connects.len() {
                        0 => "-".to_string(),
                        k if k % 2 == 1 => format!("{:.2} s", connects[k / 2]),
                        k => format!("{:.2} s", (connects[k / 2 - 1] + connects[k / 2]) / 2.0),
                    };
                    let lo = cell.iter().map(|t| t.seed).min().unwrap_or_default();
                    let hi = cell.iter().map(|t| t.seed).max().unwrap_or_default();
                    let first = cell.iter().min_by_key(|t| t.trial).copied();
                    let index = Cell {
                        channel,
                        snr_db: snr,
                        size_bytes: size,
                    }
                    .index()
                    .unwrap_or_default() as u64;
                    let base = first.map_or(0, |t| {
                        t.seed
                            .wrapping_sub(t.trial as u64)
                            .wrapping_sub(index * CELL_SEED_STRIDE)
                    });
                    let trials_n = cell.iter().map(|t| t.trial + 1).max().unwrap_or(n);
                    let _ = writeln!(
                        out,
                        "| <a id=\"{}\"></a>{}, {}, {} dB, {} | {} | {}/{} ({:.2}-{:.2}) | {} | {} | {} | 0x{:08x}-0x{:08x} | `cargo run --release -p coppa-bench -- compare --rows {} --channel {} --snr {} --size {} --trials {} --seed-base 0x{:08x} --out-dir /tmp/coppa-replay` |",
                        anchor(row.name, channel, snr, size),
                        row.name,
                        channel.name().to_uppercase(),
                        snr,
                        size_label(size),
                        n,
                        c.completed,
                        c.n,
                        c.lo,
                        c.hi,
                        drops,
                        no_connect,
                        median,
                        lo,
                        hi,
                        row.name,
                        channel.name(),
                        snr,
                        size,
                        trials_n,
                        base,
                    );
                }
            }
        }
    }
    out
}

/// The six tables (three channels by two sizes) for `rows`, measured or not,
/// followed by the replay table. Each measured cell links to its replay row.
#[must_use]
pub fn render_tables(rows: &[RowSpec], trials: &[TrialRow], version: &str) -> String {
    let mut out = render_cell_tables(rows, trials, version);
    out.push_str("### Replay\n\n");
    out.push_str(&render_replay(rows, trials));
    out
}

/// The six tables without the replay table.
#[must_use]
pub fn render_cell_tables(rows: &[RowSpec], trials: &[TrialRow], version: &str) -> String {
    let mut out = String::new();
    for channel in Channel::ALL {
        for size in SIZES {
            let _ = writeln!(
                out,
                "### {}, {} transfer\n",
                channel.title(),
                size_label(size)
            );
            out.push_str("| Row |");
            for snr in SNRS_DB {
                let _ = write!(out, " {snr} dB |");
            }
            out.push_str("\n|---|");
            for _ in SNRS_DB {
                out.push_str("---|");
            }
            out.push('\n');
            for row in rows {
                let _ = write!(out, "| coppa {version}, {} |", row.mode);
                for snr in SNRS_DB {
                    let cell = cell_trials(trials, row.name, channel, snr, size);
                    if cell.is_empty() {
                        out.push_str(" - |");
                    } else {
                        let _ = write!(
                            out,
                            " [{}](#{}) |",
                            cell_text(&cell),
                            anchor(row.name, channel, snr, size)
                        );
                    }
                }
                out.push('\n');
            }
            out.push('\n');
        }
    }
    out
}

/// Names of the generated regions of the page, in page order.
pub const REGIONS: [&str; 4] = ["tables", "replay", "blackbox-selftest", "channel-check"];

/// Render separately recorded black-box and channel measurements. A missing
/// file renders "Not recorded yet."; an unreadable or malformed one is an
/// error, so a damaged CSV cannot silently empty its region.
///
/// # Errors
///
/// Reading a present CSV fails, or it has the wrong header or row shape.
pub fn data_regions(out_dir: &std::path::Path) -> anyhow::Result<Vec<(&'static str, String)>> {
    use anyhow::Context;
    const EMPTY: &str = "Not recorded yet.";
    let blackbox_path = out_dir.join("blackbox-coppad.csv");
    let blackbox = match std::fs::read_to_string(&blackbox_path) {
        Ok(csv) => {
            let mut lines = csv.lines();
            anyhow::ensure!(
                lines.next() == Some(BLACKBOX_HEADER),
                "{}: not a black-box CSV header",
                blackbox_path.display()
            );
            for (i, row) in lines.enumerate() {
                anyhow::ensure!(
                    row.split(',').count() == BLACKBOX_COLUMNS,
                    "{}: row {} does not have {BLACKBOX_COLUMNS} fields",
                    blackbox_path.display(),
                    i + 2
                );
            }
            render_blackbox(&csv)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => EMPTY.to_string(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", blackbox_path.display())),
    };
    let channel_path = out_dir.join("channel-check.csv");
    let channel = if channel_path.exists() {
        let rows = super::channel_check::read_csv(&channel_path)
            .with_context(|| format!("reading {}", channel_path.display()))?;
        if rows.is_empty() {
            EMPTY.to_string()
        } else {
            super::channel_check::table(&rows).trim_end().to_string()
        }
    } else {
        EMPTY.to_string()
    };
    Ok(vec![
        ("blackbox-selftest", blackbox),
        ("channel-check", channel),
    ])
}

use super::blackbox::{CSV_COLUMNS as BLACKBOX_COLUMNS, CSV_HEADER as BLACKBOX_HEADER};

fn render_blackbox(csv: &str) -> String {
    let mut cells: std::collections::BTreeMap<(Channel, i32, usize), Vec<Vec<&str>>> =
        std::collections::BTreeMap::new();
    for row in csv.lines().skip(1) {
        let fields: Vec<_> = row.split(',').collect();
        if fields.len() < BLACKBOX_COLUMNS
            || fields[0] != "coppad"
            || !fields[2].starts_with("VERSION Coppa")
        {
            continue;
        }
        if let (Some(channel), Ok(snr), Ok(size)) = (
            Channel::parse(fields[5]),
            fields[6].parse(),
            fields[7].parse(),
        ) {
            cells.entry((channel, snr, size)).or_default().push(fields);
        }
    }
    if cells.is_empty() {
        return "Not recorded yet.".to_string();
    }
    let mut out = String::from("| Modem | Cell | n | completed | net B/min, mean (95% interval) | median connect | median wall | drops | cable lead | starved / overflow ticks |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for ((channel, snr, size), rows) in cells {
        let n = rows.len();
        let complete = rows.iter().filter(|r| r[13] == "complete").count();
        let drops = rows.iter().filter(|r| r[13] == "drop").count();
        let rates: Vec<f64> = rows.iter().map(|r| r[18].parse().unwrap_or(0.0)).collect();
        let ci = t_interval(&rates).expect("nonempty cell");
        let hi = if ci.hi.is_finite() {
            format!("{:.0}", ci.hi)
        } else {
            "inf".into()
        };
        let connect_times: Vec<f64> = rows.iter().filter_map(|r| r[14].parse().ok()).collect();
        let connect_text = if connect_times.is_empty() {
            "-".to_string()
        } else {
            format!("{:.1} s", median(connect_times))
        };
        let wall = median(rows.iter().filter_map(|r| r[20].parse().ok()).collect());
        let lead = median(rows.iter().filter_map(|r| r[21].parse().ok()).collect());
        let starved: u64 = rows.iter().filter_map(|r| r[22].parse::<u64>().ok()).sum();
        let overflow: u64 = rows.iter().filter_map(|r| r[23].parse::<u64>().ok()).sum();
        let version = rows[0][2].trim_start_matches("VERSION Coppa ");
        let mode = rows[0][4];
        let size = size_label(size);
        let _ = writeln!(out, "| coppad {version}, VARA-style host, {mode} | {}, {} dB, {} | {n} | {complete}/{n} | {:.0} ({:.0}-{hi}) | {connect_text} | {:.1} s | {drops} | {:.0} ms | {starved} / {overflow} |", channel.name().to_uppercase(), snr, size, ci.mean, ci.lo, wall, lead);
    }
    out.trim_end().to_string()
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn begin_marker(name: &str) -> String {
    format!("<!-- compare:begin {name} -->")
}

fn end_marker(name: &str) -> String {
    format!("<!-- compare:end {name} -->")
}

/// The generated regions for `rows` and `trials`, by name.
#[must_use]
pub fn regions(
    rows: &[RowSpec],
    trials: &[TrialRow],
    version: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("tables", render_cell_tables(rows, trials, version)),
        ("replay", render_replay(rows, trials)),
    ]
}

/// Replace the body between `<!-- compare:begin NAME -->` and
/// `<!-- compare:end NAME -->` with `body`. Text outside the markers is kept
/// byte for byte. A page without the pair is an error naming the region.
pub fn replace_region(page: &str, name: &str, body: &str) -> Result<String, String> {
    let begin = begin_marker(name);
    let end = end_marker(name);
    let start = page
        .find(&begin)
        .ok_or_else(|| format!("region \"{name}\" has no begin marker"))?
        + begin.len();
    let stop = page[start..]
        .find(&end)
        .ok_or_else(|| format!("region \"{name}\" has no end marker"))?
        + start;
    let mut out = String::with_capacity(page.len() + body.len());
    out.push_str(&page[..start]);
    out.push('\n');
    out.push_str(body.trim_end());
    out.push('\n');
    out.push_str(&page[stop..]);
    Ok(out)
}

/// A page skeleton holding every region, for a scratch out-dir.
#[must_use]
pub fn skeleton() -> String {
    let mut s = String::from("# coppa-bench compare (scratch render)\n\n");
    for name in REGIONS {
        let _ = writeln!(s, "{}\n{}\n", begin_marker(name), end_marker(name));
    }
    s
}

/// Rewrite every region of `page`.
pub fn render_page(page: &str, regions: &[(&str, String)]) -> Result<String, String> {
    let mut out = page.to_string();
    for (name, body) in regions {
        out = replace_region(&out, name, body)?;
    }
    Ok(out)
}

/// The first line where `page` and `expected` differ, with the region it
/// falls in, or `None` when they are equal.
#[must_use]
pub fn first_difference(page: &str, expected: &str) -> Option<(usize, String)> {
    if page == expected {
        return None;
    }
    let mut region = String::from("(outside the regions)");
    let mut a = page.lines();
    let mut b = expected.lines();
    let mut line = 0;
    loop {
        line += 1;
        let (x, y) = (a.next(), b.next());
        if let Some(text) = y {
            for name in REGIONS {
                if text.contains(&begin_marker(name)) {
                    region = format!("region \"{name}\"");
                }
            }
        }
        if x != y || (x.is_none() && y.is_none()) {
            return Some((line, region));
        }
    }
}

/// The version label of the rows: `coppa_version=` from `metadata.md` in
/// the out-dir when present, else this build's version.
#[must_use]
pub fn recorded_version(metadata: Option<&str>) -> String {
    metadata
        .and_then(|m| {
            m.lines()
                .find_map(|l| l.trim().strip_prefix("coppa_version="))
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::link_stack::row_spec;
    use crate::compare::trial::tests::sample_row;

    #[test]
    fn four_regions_in_page_order() {
        assert_eq!(
            REGIONS,
            ["tables", "replay", "blackbox-selftest", "channel-check"]
        );
        let page = skeleton();
        let mut last = 0;
        for region in REGIONS {
            let pos = page.find(&begin_marker(region)).unwrap();
            assert!(pos >= last);
            last = pos;
        }
    }

    #[test]
    fn missing_data_renders_the_empty_state() {
        let dir = std::env::temp_dir().join(format!("coppa-render-empty-{}", std::process::id()));
        let rendered = data_regions(&dir).unwrap();
        assert_eq!(rendered[0].1, "Not recorded yet.");
        assert_eq!(rendered[1].1, "Not recorded yet.");
    }

    #[test]
    fn malformed_data_is_an_error_not_the_empty_state() {
        let dir =
            std::env::temp_dir().join(format!("coppa-render-malformed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("channel-check.csv"), "preset,oops\ngood,1\n").unwrap();
        assert!(data_regions(&dir).is_err());
        std::fs::remove_file(dir.join("channel-check.csv")).unwrap();
        std::fs::write(
            dir.join("blackbox-coppad.csv"),
            format!("{BLACKBOX_HEADER}\ncoppad,vara,VERSION Coppa 1, build 2,x\n"),
        )
        .unwrap();
        assert!(data_regions(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn blackbox_region_has_one_row_per_cell() {
        let row = "coppad,vara,VERSION Coppa 0.16.0,abc,HF_STANDARD,wgn,30,2048,0,0x17000000,1,1,600,complete,7,2048,true,50,2457,0,58,370,0,0,0x6887c6a5,0\n";
        let text = render_blackbox(&format!("{BLACKBOX_HEADER}\n{row}{row}"));
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("| coppad "))
                .count(),
            1
        );
        assert!(text.contains("2/2"));
    }

    #[test]
    fn channel_csv_change_makes_page_stale() {
        let dir = std::env::temp_dir().join(format!("coppa-render-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = render_page(&skeleton(), &data_regions(&dir).unwrap()).unwrap();
        let csv = format!(
            "{}\ngood,48000,0x17f00000,600,1200,0.500,0.500,0.0,0.100,0.100,0.0,true\n",
            super::super::channel_check::HEADER
        );
        std::fs::write(dir.join("channel-check.csv"), csv).unwrap();
        let expected = render_page(&page, &data_regions(&dir).unwrap()).unwrap();
        assert_eq!(
            first_difference(&page, &expected).unwrap().1,
            "region \"channel-check\""
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn six_tables_with_seven_snr_columns() {
        let row = row_spec("link-l2").unwrap();
        let trials: Vec<TrialRow> = (0..8).map(sample_row).collect();
        let text = render_tables(&[row], &trials, "0.0.0");
        let tables = text
            .lines()
            .filter(|l| l.starts_with("### ") && l.ends_with(" transfer"))
            .count();
        assert_eq!(tables, 6);
        assert!(text.contains("### Replay"));
        for line in text.lines().filter(|l| l.starts_with("| Row |")) {
            assert_eq!(line.matches(" dB |").count(), 7, "{line}");
        }
        // The fixture cell: 8 equal trials, all complete.
        assert!(
            text.contains("| [1112 (1112-1112) 8/8](#cell-link-l2-mpg-20-2048) |"),
            "{text}"
        );
    }

    #[test]
    fn every_cell_links_to_its_replay_anchor() {
        let row = row_spec("link-l2").unwrap();
        let mut trials: Vec<TrialRow> = (0..8).map(sample_row).collect();
        let mut other = sample_row(0);
        other.channel = Channel::Wgn;
        other.snr_db = 0;
        other.seed = 0x170c_0000;
        trials.push(other);
        let text = render_tables(&[row], &trials, "0.0.0");
        let links: Vec<&str> = text
            .split("](#")
            .skip(1)
            .map(|s| s.split(')').next().unwrap())
            .collect();
        assert_eq!(links.len(), 2);
        for id in links {
            assert!(
                text.contains(&format!("<a id=\"{id}\"></a>")),
                "no anchor for {id}"
            );
        }
    }

    #[test]
    fn text_outside_markers_is_untouched() {
        let page = "intro\n<!-- compare:begin tables -->\nold\n<!-- compare:end tables -->\nmiddle\n<!-- compare:begin replay -->\n<!-- compare:end replay -->\noutro\n";
        let out = render_page(
            page,
            &[("tables", "new".to_string()), ("replay", "r".to_string())],
        )
        .unwrap();
        assert_eq!(
            out,
            "intro\n<!-- compare:begin tables -->\nnew\n<!-- compare:end tables -->\nmiddle\n<!-- compare:begin replay -->\nr\n<!-- compare:end replay -->\noutro\n"
        );
        // Rendering twice is a no-op.
        let again = render_page(
            &out,
            &[("tables", "new".to_string()), ("replay", "r".to_string())],
        )
        .unwrap();
        assert_eq!(again, out);
        assert!(replace_region("no markers", "tables", "x").is_err());
        assert!(render_page(&skeleton(), &[("tables", "t".into())]).is_ok());
        assert_eq!(
            recorded_version(Some("a=b\ncoppa_version=9.9.9\n")),
            "9.9.9"
        );
        assert_eq!(recorded_version(None), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn check_reports_a_stale_region() {
        let page = "intro\n<!-- compare:begin tables -->\nold\n<!-- compare:end tables -->\n";
        let expected = render_page(page, &[("tables", "new".to_string())]).unwrap();
        assert_eq!(first_difference(&expected, &expected), None);
        let (line, region) = first_difference(page, &expected).unwrap();
        assert_eq!(line, 3);
        assert_eq!(region, "region \"tables\"");
    }

    #[test]
    fn replay_row_names_command_and_seeds() {
        let row = row_spec("link-l2").unwrap();
        let trials: Vec<TrialRow> = (0..8).map(sample_row).collect();
        let replay = render_replay(&[row], &trials);
        assert!(replay.contains("0x17120000-0x17120007"), "{replay}");
        assert!(replay.contains(
            "`cargo run --release -p coppa-bench -- compare --rows link-l2 --channel mpg --snr 20 --size 2048 --trials 8 --seed-base 0x17000000 --out-dir /tmp/coppa-replay`"
        ));
        assert!(
            replay.contains("| 8 | 8/8 (0.68-1.00) | 0 | 0 | 3.30 s |"),
            "{replay}"
        );
    }

    #[test]
    fn unmeasured_cell_renders_a_dash() {
        let row = row_spec("link-l2").unwrap();
        let text = render_tables(&[row], &[], "0.0.0");
        let first_row = text
            .lines()
            .find(|l| l.starts_with("| coppa"))
            .expect("a row line");
        assert_eq!(first_row.matches(" - |").count(), 7, "{first_row}");
    }
}
