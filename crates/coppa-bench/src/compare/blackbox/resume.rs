//! Resume rules for a black-box CSV (COP-214; COP-227 added the payload).

use super::{profile::Profile, score, CSV_COLUMNS, CSV_HEADER as HEADER};
use crate::compare::grid::{Cell, Channel};
use anyhow::{Context, Result};
use std::collections::HashSet;

/// What a resumed run must share with every row already in the CSV.
pub struct CsvIdentity<'a> {
    pub profile: &'a Profile,
    pub sha: &'a str,
    pub version: &'a str,
    pub seed_base: u64,
    pub power: [f32; 2],
    pub requested: (Channel, i32, usize),
}

/// Accept only the current header. A CSV from before COP-227 has no
/// `payload_crc32` column, so its rows' payload is unknown and it is refused
/// by name.
///
/// # Errors
///
/// The first line is not [`CSV_HEADER`](super::CSV_HEADER).
pub fn check_header(csv: &str) -> Result<()> {
    let first = csv.lines().next();
    if first == Some(HEADER) {
        return Ok(());
    }
    anyhow::ensure!(
        first != HEADER.strip_suffix(",payload_crc32"),
        "black-box CSV has no payload_crc32 column, so the payload of its rows is unknown; use a fresh --out-dir"
    );
    anyhow::bail!("black-box CSV header mismatch")
}

/// The `(channel, snr, size, trial)` cells already recorded in `csv`, after
/// checking that every row was recorded with `identity` and wrote the
/// seeded payload for its own seed and size.
///
/// # Errors
///
/// The header is not current, a row is malformed or off the grid, a row's
/// identity or payload differs, or a cell is recorded twice.
pub fn recorded_cells(
    csv: &str,
    identity: &CsvIdentity<'_>,
) -> Result<HashSet<(String, i32, usize, u32)>> {
    check_header(csv)?;
    let mut seen = HashSet::new();
    for (line_no, line) in csv.lines().enumerate().skip(1) {
        let c: Vec<_> = line.split(',').collect();
        anyhow::ensure!(
            c.len() == CSV_COLUMNS,
            "malformed black-box CSV row {}",
            line_no + 1
        );
        let channel = Channel::parse(c[5]).context("unknown recorded channel")?;
        let snr: i32 = c[6].parse()?;
        let size: usize = c[7].parse()?;
        let trial: u32 = c[8].parse()?;
        let cell = Cell {
            channel,
            snr_db: snr,
            size_bytes: size,
        };
        let expected_seed = cell
            .seed(identity.seed_base, trial as usize)
            .context("recorded row is off the grid")?;
        let cap = if size == 2048 { 600 } else { 1800 };
        let powers = [c[10].parse::<f32>()?, c[11].parse::<f32>()?];
        let power_matches = (channel, snr, size) != identity.requested
            || powers.iter().zip(identity.power).all(|(&old, new)| {
                old > 0.0 && new > 0.0 && (10.0 * (old / new).log10()).abs() <= 0.1
            });
        anyhow::ensure!(
            c[0] == identity.profile.name
                && c[1] == identity.profile.adapter
                && c[2] == identity.version
                && c[3] == identity.sha
                && c[4] == identity.profile.mode
                && c[9] == format!("0x{expected_seed:x}")
                && c[12] == cap.to_string()
                && power_matches,
            "recorded trial identity mismatch on CSV row {}; use a fresh --out-dir",
            line_no + 1
        );
        // Recomputed from the row's grid seed and size, never trusted from the row.
        let expected = score::payload_crc32(&score::trial_payload(expected_seed, size));
        anyhow::ensure!(
            c[24] == expected,
            "recorded payload mismatch on CSV row {}: payload_crc32 {}, but seed 0x{expected_seed:x} and {size} bytes give {expected}; use a fresh --out-dir",
            line_no + 1,
            c[24]
        );
        anyhow::ensure!(
            seen.insert((channel.name().to_string(), snr, size, trial)),
            "duplicate black-box CSV row {}",
            line_no + 1
        );
    }
    Ok(seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Profile {
        Profile::load(&crate::compare::repo_root().join("tools/compare/coppad.toml")).unwrap()
    }

    fn identity(profile: &Profile) -> CsvIdentity<'_> {
        CsvIdentity {
            profile,
            sha: "abc",
            version: "VERSION Coppa 0.16.0",
            seed_base: 0x1700_0000,
            power: [0.02, 0.03],
            requested: (Channel::Wgn, 30, 2048),
        }
    }

    const ROW: &str = "coppad,vara,VERSION Coppa 0.16.0,abc,HF_STANDARD,wgn,30,2048,0,0x17000000,0.020000,0.030000,600,complete,7,2048,true,50,2457,0,58,355,0,0,0x6887c6a5";

    #[test]
    fn resumed_rows_require_same_seed_binary_version_and_cell_power() {
        let profile = profile();
        let identity = identity(&profile);
        let csv = format!("{HEADER}\n{ROW}\n");
        assert_eq!(recorded_cells(&csv, &identity).unwrap().len(), 1);
        assert!(recorded_cells(&csv.replace("0x17000000", "0x17000001"), &identity).is_err());
        assert!(recorded_cells(&csv.replace(",abc,", ",xyz,"), &identity).is_err());
        assert!(recorded_cells(
            &csv.replace("VERSION Coppa 0.16.0", "VERSION Other 1"),
            &identity
        )
        .is_err());
        assert!(recorded_cells(&csv.replace("0.020000", "0.040000"), &identity).is_err());
        let other_cell = csv
            .replace("wgn,30", "mpg,20")
            .replace("0x17000000", "0x17120000")
            .replace("0.020000", "0.040000")
            .replace(
                "0x6887c6a5",
                &score::payload_crc32(&score::trial_payload(0x1712_0000, 2048)),
            );
        assert!(recorded_cells(&other_cell, &identity).is_ok());
    }

    #[test]
    fn resume_refuses_a_csv_without_the_payload_column() {
        let profile = profile();
        let identity = identity(&profile);
        let legacy_header = HEADER.strip_suffix(",payload_crc32").unwrap();
        assert_eq!(legacy_header.split(',').count(), 24);
        let legacy = format!(
            "{legacy_header}\ncoppad,vara,VERSION Coppa 0.16.0,9b8b4957d0c18b1df65c23d26a97b2a855101fff94fb6902757ff0c1d6d1327e,HF_STANDARD,wgn,30,2048,0,0x17000000,0.022691,0.020689,600,complete,7.078,2048,true,51.477,2387.090,0,58.555,355,0,0\n"
        );
        for message in [
            format!("{:#}", check_header(&legacy).unwrap_err()),
            format!("{:#}", recorded_cells(&legacy, &identity).unwrap_err()),
        ] {
            assert!(message.contains("no payload_crc32 column"), "{message}");
            assert!(message.contains("use a fresh --out-dir"), "{message}");
        }
        let unrelated = format!("modem,adapter\n{ROW}\n");
        let message = format!("{:#}", check_header(&unrelated).unwrap_err());
        assert!(
            message.contains("black-box CSV header mismatch"),
            "{message}"
        );
        assert!(check_header(&format!("{HEADER}\n")).is_ok());
    }

    #[test]
    fn resume_refuses_a_different_payload_generator() {
        let profile = profile();
        let identity = identity(&profile);
        let accepted = format!("{HEADER}\n{ROW}\n");
        assert_eq!(recorded_cells(&accepted, &identity).unwrap().len(), 1);
        // The COP-214 ramp, another seed's bytes, and this seed's bytes at
        // another size.
        let ramp: Vec<u8> = (0..2048u64)
            .map(|i| ((0x1700_0000u64 + i) & 0xff) as u8)
            .collect();
        for other in [
            score::payload_crc32(&ramp),
            score::payload_crc32(&score::trial_payload(0x1700_0001, 2048)),
            score::payload_crc32(&score::trial_payload(0x1700_0000, 16384)),
        ] {
            assert_ne!(other, "0x6887c6a5");
            let csv = accepted.replace("0x6887c6a5", &other);
            let message = format!("{:#}", recorded_cells(&csv, &identity).unwrap_err());
            assert!(
                message.contains("recorded payload mismatch on CSV row 2"),
                "{message}"
            );
            assert!(
                message.contains(&format!("payload_crc32 {other}")),
                "{message}"
            );
            assert!(message.contains("0x6887c6a5"), "{message}");
            assert!(message.contains("use a fresh --out-dir"), "{message}");
        }
        let short_row = ROW.strip_suffix(",0x6887c6a5").unwrap();
        let message = format!(
            "{:#}",
            recorded_cells(&format!("{HEADER}\n{short_row}\n"), &identity).unwrap_err()
        );
        assert!(
            message.contains("malformed black-box CSV row 2"),
            "{message}"
        );
    }
}
