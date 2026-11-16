//! Two unmodified modem processes driven through a measured userspace audio cable.

#[cfg(all(feature = "cpal-backend", target_os = "linux"))]
mod cable;
pub mod guard;
pub mod integrity;
pub mod profile;
pub mod relay;
#[cfg(all(feature = "cpal-backend", target_os = "linux"))]
mod station;
pub mod tnc;

use clap::Args;
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
