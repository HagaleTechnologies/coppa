//! Report generation: CSV rows and a markdown summary.

use std::fmt::Write as _;

use crate::metrics::MeasurementPoint;

/// CSV header line.
pub const CSV_HEADER: &str = "level,mode,channel,snr_db,trials,frame_errors,fer,ber,goodput_bps";

/// Format one `MeasurementPoint` as a CSV row (no trailing newline).
pub fn csv_row(p: &MeasurementPoint) -> String {
    format!(
        "{},{},{},{:.1},{},{},{:.6},{:.8},{:.2}",
        p.level,
        p.mode_name,
        p.channel,
        p.snr_db,
        p.trials,
        p.frame_errors,
        p.fer,
        p.ber,
        p.goodput_bps
    )
}

/// Build a full CSV document from measurement points.
pub fn to_csv(points: &[MeasurementPoint]) -> String {
    let mut out = String::new();
    out.push_str(CSV_HEADER);
    out.push('\n');
    for p in points {
        out.push_str(&csv_row(p));
        out.push('\n');
    }
    out
}

/// Lowest SNR at which a level's FER is at or below `target_fer`.
pub fn fer_threshold(points: &[MeasurementPoint], level: u8, target_fer: f64) -> Option<f32> {
    points
        .iter()
        .filter(|p| p.level == level && p.fer <= target_fer)
        .map(|p| p.snr_db)
        .min_by(|a, b| a.total_cmp(b))
}

/// Build a markdown summary table: FER thresholds + peak goodput per mode.
pub fn to_markdown(points: &[MeasurementPoint], channel_title: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## {} channel\n", channel_title);
    let _ = writeln!(
        out,
        "| Mode | SNR @ FER=10% | SNR @ FER=1% | Peak goodput (bps) |"
    );
    let _ = writeln!(
        out,
        "|------|---------------|--------------|--------------------|"
    );

    let mut levels: Vec<u8> = points.iter().map(|p| p.level).collect();
    levels.sort_unstable();
    levels.dedup();

    for level in levels {
        let name = points
            .iter()
            .find(|p| p.level == level)
            .map(|p| p.mode_name)
            .unwrap_or("?");
        let t10 = fer_threshold(points, level, 0.10)
            .map(|s| format!("{:.1} dB", s))
            .unwrap_or_else(|| "—".to_string());
        let t1 = fer_threshold(points, level, 0.01)
            .map(|s| format!("{:.1} dB", s))
            .unwrap_or_else(|| "—".to_string());
        let peak = points
            .iter()
            .filter(|p| p.level == level)
            .map(|p| p.goodput_bps)
            .fold(0.0f64, f64::max);
        let _ = writeln!(out, "| {} | {} | {} | {:.0} |", name, t10, t1, peak);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MeasurementPoint;

    fn point(level: u8, snr: f32, fer: f64, goodput: f64) -> MeasurementPoint {
        MeasurementPoint {
            level,
            mode_name: "TEST",
            channel: "awgn",
            snr_db: snr,
            trials: 100,
            frame_errors: (fer * 100.0) as usize,
            fer,
            ber: 0.0,
            goodput_bps: goodput,
        }
    }

    #[test]
    fn csv_row_has_nine_fields() {
        let row = csv_row(&point(2, 10.0, 0.0, 968.0));
        assert_eq!(row.split(',').count(), 9);
    }

    #[test]
    fn fer_threshold_picks_lowest_passing_snr() {
        let pts = vec![
            point(2, 0.0, 0.5, 0.0),
            point(2, 4.0, 0.05, 500.0),
            point(2, 8.0, 0.0, 968.0),
        ];
        assert_eq!(fer_threshold(&pts, 2, 0.10), Some(4.0));
        assert_eq!(fer_threshold(&pts, 2, 0.01), Some(8.0));
    }

    #[test]
    fn markdown_contains_a_row_per_mode() {
        let pts = vec![point(2, 8.0, 0.0, 968.0), point(3, 10.0, 0.0, 1900.0)];
        let md = to_markdown(&pts, "AWGN");
        assert!(md.contains("AWGN channel"));
        assert_eq!(md.matches("| TEST |").count(), 2);
    }
}
