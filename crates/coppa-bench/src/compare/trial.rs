//! COP-170: one trial's record, its frozen CSV form, and the pure clock that
//! classifies a trial from its event times.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use super::grid::Channel;

/// The trial CSV header. Frozen: a change breaks every recorded file.
pub const HEADER: &str = "row,stack,level,robust_floor,channel,snr_db,size_bytes,trial,seed,ref_power,cap_s,outcome,connect_s,delivered_bytes,exact,transfer_s,net_bytes_per_min,drops,virtual_s";

const FIELDS: usize = 19;

/// Seconds from the dial within which the caller must report CONNECTED.
pub const CONNECT_DEADLINE_S: f64 = 120.0;

/// Transfer cap after the write: 600 s up to 2 KB (the 2020 study's longest
/// run), 1800 s above.
#[must_use]
pub fn cap_s(size_bytes: usize) -> f64 {
    if size_bytes <= 2048 {
        600.0
    } else {
        1800.0
    }
}

/// How a trial ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Every byte arrived.
    Complete,
    /// Either end left the session after the caller's CONNECTED, before the
    /// last byte.
    Drop,
    /// Still alive at the transfer cap.
    Cap,
    /// No caller CONNECTED within [`CONNECT_DEADLINE_S`], or the handshake failed.
    NoConnect,
}

impl Outcome {
    /// CSV name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Complete => "complete",
            Outcome::Drop => "drop",
            Outcome::Cap => "cap",
            Outcome::NoConnect => "no_connect",
        }
    }

    fn parse(s: &str) -> Option<Outcome> {
        [
            Outcome::Complete,
            Outcome::Drop,
            Outcome::Cap,
            Outcome::NoConnect,
        ]
        .into_iter()
        .find(|o| o.name() == s)
    }
}

/// Event times of one trial, in seconds since the dial.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrialEvents {
    /// The caller's CONNECTED (also when the payload is written).
    pub connected_at: Option<f64>,
    /// When the last payload byte arrived.
    pub completed_at: Option<f64>,
    /// When either end left the session after connecting.
    pub dropped_at: Option<f64>,
    /// When the trial stopped.
    pub stopped_at: f64,
    /// Payload bytes delivered.
    pub delivered: usize,
}

/// What the clock derives from [`TrialEvents`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classified {
    /// Outcome.
    pub outcome: Outcome,
    /// Dial to the caller's CONNECTED.
    pub connect_s: Option<f64>,
    /// Write to the last byte, or to the end of the trial when it did not complete.
    pub transfer_s: f64,
    /// Net bytes per minute; connect time is excluded.
    pub net_bytes_per_min: f64,
    /// 1 for a drop, else 0.
    pub drops: u32,
}

/// Classifies a trial. Pure: event times in, outcome and rate out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrialClock {
    /// Payload size in bytes.
    pub size_bytes: usize,
    /// Transfer cap after the write.
    pub cap_s: f64,
}

impl TrialClock {
    /// Clock for a transfer of `size_bytes` with the standard cap.
    #[must_use]
    pub fn new(size_bytes: usize) -> Self {
        Self {
            size_bytes,
            cap_s: cap_s(size_bytes),
        }
    }

    /// Outcome, connect time, transfer time and rate.
    #[must_use]
    pub fn classify(&self, ev: &TrialEvents) -> Classified {
        let Some(connected) = ev.connected_at.filter(|&c| c <= CONNECT_DEADLINE_S) else {
            return Classified {
                outcome: Outcome::NoConnect,
                connect_s: None,
                transfer_s: 0.0,
                net_bytes_per_min: 0.0,
                drops: 0,
            };
        };
        let (outcome, end) = match (ev.completed_at, ev.dropped_at) {
            (Some(done), _) if ev.delivered >= self.size_bytes => (Outcome::Complete, done),
            (_, Some(dropped)) => (Outcome::Drop, dropped),
            _ => (Outcome::Cap, ev.stopped_at.min(connected + self.cap_s)),
        };
        let transfer_s = (end - connected).max(0.0);
        let delivered = ev.delivered.min(self.size_bytes);
        let net_bytes_per_min = if transfer_s > 0.0 {
            delivered as f64 * 60.0 / transfer_s
        } else {
            0.0
        };
        Classified {
            outcome,
            connect_s: Some(connected),
            transfer_s,
            net_bytes_per_min,
            drops: u32::from(outcome == Outcome::Drop),
        }
    }
}

/// One trial row of the comparison CSV.
#[derive(Debug, Clone, PartialEq)]
pub struct TrialRow {
    /// Row name, e.g. `link-l2`.
    pub row: String,
    /// Stack, e.g. `link`.
    pub stack: String,
    /// Speed level the row starts at.
    pub level: u8,
    /// Whether the robust floor is enabled.
    pub robust_floor: bool,
    /// Channel.
    pub channel: Channel,
    /// SNR (dB).
    pub snr_db: i32,
    /// Payload size.
    pub size_bytes: usize,
    /// Trial index within the cell.
    pub trial: usize,
    /// Seed.
    pub seed: u64,
    /// Clean frame power the SNR refers to.
    pub ref_power: f64,
    /// Transfer cap.
    pub cap_s: f64,
    /// Outcome.
    pub outcome: Outcome,
    /// Connect time; empty when the caller never connected.
    pub connect_s: Option<f64>,
    /// Bytes delivered.
    pub delivered_bytes: usize,
    /// Delivered bytes equal the payload prefix.
    pub exact: bool,
    /// Transfer time.
    pub transfer_s: f64,
    /// Net bytes per minute.
    pub net_bytes_per_min: f64,
    /// Sessions lost after connecting.
    pub drops: u32,
    /// Virtual seconds the trial ran.
    pub virtual_s: f64,
}

/// The identity of a trial within a results file.
pub type TrialKey = (String, Channel, i32, usize, usize);

impl TrialRow {
    /// Key used to match recorded trials against a plan.
    #[must_use]
    pub fn key(&self) -> TrialKey {
        (
            self.row.clone(),
            self.channel,
            self.snr_db,
            self.size_bytes,
            self.trial,
        )
    }

    /// One CSV line without its newline. Times have 2 decimals, rates 1 and
    /// the reference power 6, so a re-run reproduces the bytes.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut s = String::new();
        let connect = self
            .connect_s
            .map(|c| format!("{c:.2}"))
            .unwrap_or_default();
        let _ = write!(
            s,
            "{},{},{},{},{},{},{},{},0x{:08x},{:.6},{:.0},{},{},{},{},{:.2},{:.1},{},{:.2}",
            self.row,
            self.stack,
            self.level,
            self.robust_floor,
            self.channel.name(),
            self.snr_db,
            self.size_bytes,
            self.trial,
            self.seed,
            self.ref_power,
            self.cap_s,
            self.outcome.name(),
            connect,
            self.delivered_bytes,
            self.exact,
            self.transfer_s,
            self.net_bytes_per_min,
            self.drops,
            self.virtual_s,
        );
        s
    }

    /// Parse one CSV line.
    pub fn parse_line(line: &str) -> Result<TrialRow> {
        let f: Vec<&str> = line.trim_end_matches('\r').split(',').collect();
        if f.len() != FIELDS {
            bail!("expected {FIELDS} fields, found {}", f.len());
        }
        let seed_text = f[8]
            .strip_prefix("0x")
            .with_context(|| format!("seed {:?} is not 0x-prefixed hex", f[8]))?;
        Ok(TrialRow {
            row: f[0].to_string(),
            stack: f[1].to_string(),
            level: f[2].parse().context("level")?,
            robust_floor: f[3].parse().context("robust_floor")?,
            channel: Channel::parse(f[4]).with_context(|| format!("channel {:?}", f[4]))?,
            snr_db: f[5].parse().context("snr_db")?,
            size_bytes: f[6].parse().context("size_bytes")?,
            trial: f[7].parse().context("trial")?,
            seed: u64::from_str_radix(seed_text, 16).context("seed")?,
            ref_power: f[9].parse().context("ref_power")?,
            cap_s: f[10].parse().context("cap_s")?,
            outcome: Outcome::parse(f[11]).with_context(|| format!("outcome {:?}", f[11]))?,
            connect_s: if f[12].is_empty() {
                None
            } else {
                Some(f[12].parse().context("connect_s")?)
            },
            delivered_bytes: f[13].parse().context("delivered_bytes")?,
            exact: f[14].parse().context("exact")?,
            transfer_s: f[15].parse().context("transfer_s")?,
            net_bytes_per_min: f[16].parse().context("net_bytes_per_min")?,
            drops: f[17].parse().context("drops")?,
            virtual_s: f[18].parse().context("virtual_s")?,
        })
    }
}

/// Parse a trial CSV. A last line without its newline, or one that does not
/// parse, is an interrupted write and is dropped; any other bad line is an
/// error naming the line.
pub fn parse_csv(text: &str) -> Result<Vec<TrialRow>> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // A complete file ends in '\n', so the last piece is empty; anything else
    // there is an interrupted append and is dropped.
    lines.pop();
    let mut rows = Vec::new();
    let Some((&header, body)) = lines.split_first() else {
        // Empty file, or only a partial header line.
        return Ok(rows);
    };
    if header.trim_end_matches('\r') != HEADER {
        bail!("header does not match the frozen trial header");
    }
    let last = body.len().saturating_sub(1);
    for (i, line) in body.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match TrialRow::parse_line(line) {
            Ok(row) => rows.push(row),
            // A newline-terminated last line that does not parse is also an
            // interrupted write (a torn append); drop it.
            Err(_) if i == last => {}
            Err(e) => return Err(e.context(format!("line {}", i + 2))),
        }
    }
    Ok(rows)
}

/// Read a trial CSV; a missing file reads as no trials.
pub fn read_csv(path: &Path) -> Result<Vec<TrialRow>> {
    match fs::read_to_string(path) {
        Ok(text) => parse_csv(&text).with_context(|| format!("reading {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// The whole file for `rows`, header first, each line newline-terminated.
#[must_use]
pub fn to_csv(rows: &[TrialRow]) -> String {
    let mut s = String::with_capacity(64 * (rows.len() + 1));
    s.push_str(HEADER);
    s.push('\n');
    for r in rows {
        s.push_str(&r.to_line());
        s.push('\n');
    }
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_row(trial: usize) -> TrialRow {
        TrialRow {
            row: "link-l2".into(),
            stack: "link".into(),
            level: 2,
            robust_floor: false,
            channel: Channel::Mpg,
            snr_db: 20,
            size_bytes: 2048,
            trial,
            seed: 0x1712_0000 + trial as u64,
            ref_power: 0.021_82,
            cap_s: 600.0,
            outcome: Outcome::Complete,
            connect_s: Some(3.3),
            delivered_bytes: 2048,
            exact: true,
            transfer_s: 110.5,
            net_bytes_per_min: 1112.0,
            drops: 0,
            virtual_s: 113.8,
        }
    }

    #[test]
    fn header_is_frozen() {
        assert_eq!(
            HEADER,
            "row,stack,level,robust_floor,channel,snr_db,size_bytes,trial,seed,ref_power,cap_s,outcome,connect_s,delivered_bytes,exact,transfer_s,net_bytes_per_min,drops,virtual_s"
        );
        assert_eq!(HEADER.split(',').count(), FIELDS);
    }

    #[test]
    fn csv_round_trip() {
        let mut no_connect = sample_row(1);
        no_connect.outcome = Outcome::NoConnect;
        no_connect.connect_s = None;
        no_connect.delivered_bytes = 0;
        let rows = vec![sample_row(0), no_connect];
        let text = to_csv(&rows);
        assert!(text.contains(
            "link-l2,link,2,false,mpg,20,2048,0,0x17120000,0.021820,600,complete,3.30,2048,true,110.50,1112.0,0,113.80\n"
        ));
        assert!(text.contains(",no_connect,,0,"));
        let back = parse_csv(&text).unwrap();
        assert_eq!(back, rows);
        assert_eq!(to_csv(&back), text);
    }

    #[test]
    fn truncated_last_line_is_dropped_on_resume() {
        let full = to_csv(&[sample_row(0), sample_row(1)]);
        let cut = &full[..full.len() - 20];
        let rows = parse_csv(cut).unwrap();
        assert_eq!(rows, vec![sample_row(0)]);
        // A bad line in the middle is an error, not a silent drop.
        let mut lines: Vec<&str> = full.lines().collect();
        lines.insert(2, "garbage");
        let bad = lines.join("\n") + "\n";
        assert!(parse_csv(&bad).is_err());
        // A file with another header is refused.
        assert!(parse_csv("a,b\n").is_err());
        assert!(parse_csv("").unwrap().is_empty());
    }

    #[test]
    fn clock_classifies_complete_drop_cap_and_no_connect() {
        let clock = TrialClock::new(2048);
        let complete = clock.classify(&TrialEvents {
            connected_at: Some(3.0),
            completed_at: Some(63.0),
            dropped_at: None,
            stopped_at: 63.0,
            delivered: 2048,
        });
        assert_eq!(complete.outcome, Outcome::Complete);
        assert_eq!(complete.drops, 0);
        let drop = clock.classify(&TrialEvents {
            connected_at: Some(3.0),
            completed_at: None,
            dropped_at: Some(123.0),
            stopped_at: 123.0,
            delivered: 600,
        });
        assert_eq!(drop.outcome, Outcome::Drop);
        assert_eq!(drop.drops, 1);
        assert!((drop.net_bytes_per_min - 300.0).abs() < 1e-9);
        let cap = clock.classify(&TrialEvents {
            connected_at: Some(3.0),
            completed_at: None,
            dropped_at: None,
            stopped_at: 603.0,
            delivered: 1000,
        });
        assert_eq!(cap.outcome, Outcome::Cap);
        assert!((cap.transfer_s - 600.0).abs() < 1e-9);
        assert!((cap.net_bytes_per_min - 100.0).abs() < 1e-9);
        let none = clock.classify(&TrialEvents {
            stopped_at: 120.0,
            ..TrialEvents::default()
        });
        assert_eq!(none.outcome, Outcome::NoConnect);
        assert_eq!(none.connect_s, None);
        assert_eq!(none.net_bytes_per_min, 0.0);
        let late = clock.classify(&TrialEvents {
            connected_at: Some(130.0),
            stopped_at: 130.0,
            ..TrialEvents::default()
        });
        assert_eq!(late.outcome, Outcome::NoConnect);
        assert_eq!(TrialClock::new(16384).cap_s, 1800.0);
    }

    #[test]
    fn rate_excludes_connect_time() {
        let clock = TrialClock::new(2048);
        let slow_connect = clock.classify(&TrialEvents {
            connected_at: Some(60.0),
            completed_at: Some(120.0),
            dropped_at: None,
            stopped_at: 120.0,
            delivered: 2048,
        });
        assert_eq!(slow_connect.connect_s, Some(60.0));
        assert!((slow_connect.transfer_s - 60.0).abs() < 1e-9);
        assert!((slow_connect.net_bytes_per_min - 2048.0).abs() < 1e-9);
    }
}
