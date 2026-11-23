//! Two unmodified modem processes driven through a measured userspace audio cable.

#[cfg(all(feature = "cpal-backend", target_os = "linux"))]
mod cable;
pub mod guard;
pub mod integrity;
pub mod profile;
pub mod relay;
pub mod resume;
pub mod score;
#[cfg(all(feature = "cpal-backend", target_os = "linux"))]
mod station;
pub mod tnc;

use clap::Args;

/// The black-box trial CSV header (COP-170 Design section 5; COP-227 appended `payload_crc32`).
pub const CSV_HEADER: &str = "modem,adapter,version_reply,binary_sha256,mode,channel,snr_db,size_bytes,trial,seed,ref_power_a,ref_power_b,cap_s,outcome,connect_s,delivered_bytes,exact,transfer_s,net_bytes_per_min,drops,wall_s,cable_latency_ms,starved_ticks,overflow_ticks,payload_crc32";
/// The number of fields in [`CSV_HEADER`] and in every black-box CSV row.
pub const CSV_COLUMNS: usize = 25;
use std::path::PathBuf;

#[derive(Debug, Clone, Args)]
pub struct BlackboxArgs {
    /// TOML profile for the unmodified modem binary.
    #[arg(long)]
    pub modem: PathBuf,
    #[arg(long, default_value = "wgn")]
    pub channel: String,
    #[arg(long, default_value_t = 30)]
    pub snr: i32,
    #[arg(long, default_value_t = 2048)]
    pub size: usize,
    #[arg(long, default_value_t = 1)]
    pub trials: u32,
    #[arg(long, default_value = "0x17000000", value_parser = parse_seed)]
    pub seed_base: u64,
    #[arg(long, default_value_t = 1)]
    pub jobs: usize,
    #[arg(long)]
    pub ref_power: Option<String>,
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
}

fn parse_seed(s: &str) -> Result<u64, String> {
    if let Some(hex) = s.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|e| e.to_string())
    } else {
        s.parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())
    }
}

pub fn run(args: &BlackboxArgs) -> anyhow::Result<()> {
    #[cfg(all(feature = "cpal-backend", target_os = "linux"))]
    {
        runtime::run(args)
    }
    #[cfg(not(all(feature = "cpal-backend", target_os = "linux")))]
    {
        let _ = args;
        anyhow::bail!("this coppa-bench was built without the black-box harness; rebuild on Linux with --features cpal-backend")
    }
}

/// Run the integrity probe, then the trials only if it passed. A failed
/// probe returns before `trials` is called, so no trial runs and no CSV row
/// is written (COP-170 AC10).
pub fn gated<T>(
    probe: impl FnOnce() -> anyhow::Result<integrity::Metrics>,
    trials: impl FnOnce(integrity::Metrics) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    use anyhow::Context;
    let metrics = probe().context("integrity probe failed; no trial recorded")?;
    eprintln!(
        "compare blackbox: integrity probe: correlation {:.6}, level {:+.2} dB, cable lead {:.0} ms: pass",
        metrics.correlation, metrics.level_db, metrics.cable_latency_ms
    );
    trials(metrics)
}

pub fn probe_child() -> anyhow::Result<()> {
    #[cfg(all(feature = "cpal-backend", target_os = "linux"))]
    {
        runtime::probe_child()
    }
    #[cfg(not(all(feature = "cpal-backend", target_os = "linux")))]
    {
        anyhow::bail!("this coppa-bench was built without the black-box harness; rebuild on Linux with --features cpal-backend")
    }
}

#[cfg(all(feature = "cpal-backend", target_os = "linux"))]
mod runtime;

#[cfg(test)]
mod tests {
    use super::*;

    /// A broadband, nonperiodic probe, as the harness sends.
    fn probe_signal() -> Vec<f32> {
        let mut state = 0x1700_2140u64;
        (0..2000)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                ((state >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.2
            })
            .collect()
    }

    #[test]
    fn trials_refused_when_integrity_fails() {
        // The shape the COP-214 planning probe measured through ALSA `plug`:
        // a third of the samples zeroed.
        let sent = probe_signal();
        let mut received = sent.clone();
        for v in received.iter_mut().step_by(3) {
            *v = 0.0;
        }
        let mut ran = false;
        let error = gated(
            || integrity::analyse(&sent, &received, 1000, 0, 0),
            |_| {
                ran = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(!ran, "a trial ran after a failed integrity probe");
        let message = format!("{error:#}");
        assert!(message.contains("no trial recorded"), "{message}");
        assert!(message.contains("integrity probe FAIL"), "{message}");

        // Starved ticks in the probe window also refuse the trials.
        let mut ran = false;
        assert!(gated(
            || integrity::analyse(&sent, &sent, 1000, 1, 0),
            |_| {
                ran = true;
                Ok(())
            },
        )
        .is_err());
        assert!(!ran);
    }

    #[test]
    fn csv_header_has_csv_columns_fields() {
        assert_eq!(CSV_HEADER.split(',').count(), CSV_COLUMNS);
        assert!(CSV_HEADER.ends_with(",payload_crc32"));
    }

    #[test]
    fn trials_run_after_a_passing_probe() {
        let sent = probe_signal();
        let mut lead = vec![0.0f32; 37];
        lead.extend_from_slice(&sent);
        lead.extend(std::iter::repeat_n(0.0, 100));
        let lead_ms = gated(
            || integrity::analyse(&sent, &lead, 1000, 0, 0),
            |metrics| Ok(metrics.cable_latency_ms),
        )
        .unwrap();
        assert!((lead_ms - 37.0).abs() < 1e-9, "{lead_ms}");
    }
}
