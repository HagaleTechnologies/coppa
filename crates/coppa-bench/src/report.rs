//! Report generation: CSV rows and a markdown summary.

use std::fmt::Write as _;

use crate::metrics::MeasurementPoint;

/// CSV header line.
pub const CSV_HEADER: &str =
    "level,mode,channel,snr_db,trials,frame_errors,fer,fer_lo,fer_hi,ber,goodput_bps";

/// Format one `MeasurementPoint` as a CSV row (no trailing newline).
pub fn csv_row(p: &MeasurementPoint) -> String {
    format!(
        "{},{},{},{:.1},{},{},{:.6},{:.6},{:.6},{:.8},{:.2}",
        p.level,
        p.mode_name,
        p.channel,
        p.snr_db,
        p.trials,
        p.frame_errors,
        p.fer,
        p.fer_lo,
        p.fer_hi,
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

/// Lowest SNR at which a level's FER is at or below `target_fer` **with 95%
/// confidence** (the Wilson upper bound must clear the target, so "0 failures
/// in 10 trials" cannot claim FER<=1%).
pub fn fer_threshold(points: &[MeasurementPoint], level: u8, target_fer: f64) -> Option<f32> {
    points
        .iter()
        .filter(|p| p.level == level && p.fer_hi <= target_fer)
        .map(|p| p.snr_db)
        .min_by(|a, b| a.total_cmp(b))
}

/// Build a markdown summary table: FER thresholds + peak goodput per mode.
pub fn to_markdown(points: &[MeasurementPoint], channel_title: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## {} channel\n", channel_title);
    let _ = writeln!(
        out,
        "| Mode | SNR @ FER≤10% (95% CI) | SNR @ FER≤1% (95% CI) | Peak goodput (bps) |"
    );
    let _ = writeln!(
        out,
        "|------|------------------------|-----------------------|--------------------|"
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
            fer_lo: fer * 0.5,
            fer_hi: (fer * 1.5).clamp(0.02, 1.0),
            ber: 0.0,
            goodput_bps: goodput,
        }
    }

    #[test]
    fn fer_threshold_picks_lowest_passing_snr() {
        let mut pts = vec![
            point(2, 0.0, 0.5, 0.0),
            point(2, 4.0, 0.05, 500.0),
            point(2, 8.0, 0.0, 968.0),
        ];
        // Set fer_hi values to match the test intent:
        // - point at 0.0 dB has fer_hi > 0.10 (fails 10% target)
        // - point at 4.0 dB has fer_hi <= 0.10 but > 0.01 (passes 10%, fails 1%)
        // - point at 8.0 dB has fer_hi <= 0.01 (passes both)
        pts[0].fer_hi = 0.25;
        pts[1].fer_hi = 0.08;
        pts[2].fer_hi = 0.008;

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

    #[test]
    fn fer_threshold_uses_upper_confidence_bound() {
        // Point FER 0.0 but wide CI (0/10 trials → hi ≈ 0.28) must NOT count as
        // meeting a 10% target; a tight CI (0/500 → hi ≈ 0.0077) must.
        let mut loose = point(2, 4.0, 0.0, 500.0);
        loose.fer_hi = 0.28;
        let mut tight = point(2, 8.0, 0.0, 968.0);
        tight.fer_hi = 0.0077;
        let pts = vec![loose, tight];
        assert_eq!(fer_threshold(&pts, 2, 0.10), Some(8.0));
    }

    #[test]
    fn csv_includes_ci_columns() {
        let row = csv_row(&point(2, 10.0, 0.0, 968.0));
        assert_eq!(row.split(',').count(), 11);
        assert!(CSV_HEADER.contains("fer_lo") && CSV_HEADER.contains("fer_hi"));
    }
}
