//! Apply the pinned COP-116 AWGN convention to clean Direwolf audio.
use anyhow::{ensure, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    clean: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut input = hound::WavReader::open(args.clean)?;
    let spec = input.spec();
    ensure!(
        spec.channels == 1
            && spec.sample_rate == 48_000
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "expected mono 16-bit PCM at 48000 Hz"
    );
    let clean: Vec<f32> = input
        .samples::<i16>()
        .map(|s| s.map(|v| v as f32 / 32768.0))
        .collect::<Result<_, _>>()?;
    let noisy = coppa_channel::awgn_seeded(&clean, 10.0, 42);
    let mut output = hound::WavWriter::create(&args.out, spec)?;
    for sample in &noisy {
        output.write_sample((sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16)?;
    }
    output.finalize()?;
    println!(
        "{} samples, 48000 Hz, mono 16-bit; AWGN 10 dB, seed 42",
        noisy.len()
    );
    Ok(())
}
