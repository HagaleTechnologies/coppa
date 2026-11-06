//! Compare coppa G3RUH 9600 reception with Direwolf on a 16-bit PCM WAV (COP-116).
//!
//! Uses the TNC's 48 kHz input and 1024-sample blocks at five start offsets.
//! Direwolf runs with MODEM 9600; atest runs with -B 9600. The report goes to stdout.
//! The 95 percent bar is a measurement and never changes the process exit status.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;

use coppa_bench::afsk_direwolf::{
    coppa_counts_by_offset, load_channel0, open_pcm16, run_atest, run_direwolf, DemodKind, Report,
};

#[derive(Debug, Parser)]
#[command(
    name = "g3ruh_vs_direwolf",
    about = "Count AX.25 packets in a WAV with coppa's G3RUH 9600 demodulator and with Direwolf (COP-116)"
)]
struct Args {
    /// 16-bit PCM WAV to decode (channel 0 is used)
    #[arg(long)]
    wav: PathBuf,
    /// The direwolf program
    #[arg(long, default_value = "direwolf")]
    direwolf: PathBuf,
    /// Direwolf's atest program
    #[arg(long, default_value = "atest")]
    atest: PathBuf,
    /// Keep direwolf.conf, direwolf.txt and atest.txt here
    #[arg(long)]
    log_dir: Option<PathBuf>,
}

/// A scratch directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Creates `dir` readable only by this user, failing if it already exists, so a directory or
/// symlink someone else planted under the shared temp dir is never written into.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

fn run(args: &Args) -> Result<Report> {
    // Direwolf needs 16-bit PCM; check before coppa's five passes, not after.
    drop(open_pcm16(&args.wav)?);
    let (samples, input) = load_channel0(&args.wav)?;
    let offsets = vec![0, 1, 2, 3, 4];
    let coppa_by_offset =
        coppa_counts_by_offset(&samples, input.rate, &offsets, DemodKind::G3ruh9600)?;
    drop(samples);

    let temp;
    let work_dir: &Path = match &args.log_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            dir
        }
        None => {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir()
                .join(format!("g3ruh_vs_direwolf_{}_{nanos}", std::process::id()));
            create_private_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
            temp = TempDir(dir);
            &temp.0
        }
    };

    let direwolf = run_direwolf(&args.direwolf, &args.wav, work_dir, DemodKind::G3ruh9600)?;
    let atest = run_atest(&args.atest, &args.wav, &["-B", "9600"])?;
    if let Some(dir) = &args.log_dir {
        for (name, bytes) in [
            ("direwolf.txt", &direwolf.output),
            ("atest.txt", &atest.output),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(Report {
        kind: DemodKind::G3ruh9600,
        input,
        offsets,
        coppa_by_offset,
        direwolf,
        atest,
    })
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("g3ruh_vs_direwolf: error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
