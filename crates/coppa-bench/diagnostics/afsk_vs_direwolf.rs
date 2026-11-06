//! COP-112: count the AX.25 packets in a WAV with coppa's AFSK 1200 demodulator and with Direwolf,
//! and print coppa's count as a percentage of Direwolf's.
//!
//! ```text
//! cargo run --release -q -p coppa-bench --example afsk_vs_direwolf -- --wav track2.wav
//! ```
//!
//! coppa's side is the TNC's receive chain after the sound card: the 48 kHz resampler and the
//! demodulator in 1024-sample reads, at eight start offsets. Direwolf's side is `direwolf -c
//! <io-only config> -t 0` with channel 0 on stdin, and `atest <wav>` with no options. The logic
//! and the report format live in `coppa_bench::afsk_direwolf`. `tools/tnc-bench/run.sh` fetches
//! WA8LMF TNC Test CD track 2 and runs this program on it; see `tools/tnc-bench/README.md`.
//!
//! The report goes to stdout. Any error prints one line to stderr and exits 1. The 95 percent bar
//! never changes the exit status.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;

use coppa_bench::afsk_direwolf::{
    coppa_counts_by_offset, load_channel0, open_pcm16, run_atest, run_direwolf, DemodKind, Report,
    START_OFFSETS,
};

#[derive(Debug, Parser)]
#[command(
    name = "afsk_vs_direwolf",
    about = "Count AX.25 packets in a WAV with coppa's AFSK 1200 demodulator and with Direwolf (COP-112)"
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
    // Direwolf needs 16-bit PCM; check before coppa's eight passes, not after.
    drop(open_pcm16(&args.wav)?);
    let (samples, input) = load_channel0(&args.wav)?;
    let offsets = START_OFFSETS.to_vec();
    let coppa_by_offset =
        coppa_counts_by_offset(&samples, input.rate, &offsets, DemodKind::Afsk1200)?;
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
                .join(format!("afsk_vs_direwolf_{}_{nanos}", std::process::id()));
            create_private_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
            temp = TempDir(dir);
            &temp.0
        }
    };

    let direwolf = run_direwolf(&args.direwolf, &args.wav, work_dir, DemodKind::Afsk1200)?;
    let atest = run_atest(&args.atest, &args.wav, &[])?;
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
        kind: DemodKind::Afsk1200,
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
            eprintln!("afsk_vs_direwolf: error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
