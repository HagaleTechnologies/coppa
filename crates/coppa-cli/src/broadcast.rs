//! Operator-facing file broadcasts over an already configured OFDM KISS service.
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use coppa_host::kiss::{kiss_encode_data, KissDecoder, KissFrame};
use coppa_protocol::broadcast_file::{encode_file, FileReceiver};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Submit repeated copies of a file. Delivery is unconfirmed.
    Send {
        file: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8100")]
        kiss: String,
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
        repeat: u32,
        #[arg(long, default_value_t = 30000, value_parser = clap::value_parser!(u64).range(1..))]
        interval_ms: u64,
    },
    /// Listen without transmitting and save the first verified file.
    Receive {
        #[arg(long, default_value = "127.0.0.1:8100")]
        kiss: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..))]
        idle_timeout: u64,
    },
}

pub(crate) fn run(command: Command, verbosity: super::Verbosity) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        tokio::select! {
            result = execute(command, verbosity) => result,
            signal = tokio::signal::ctrl_c() => {
                signal?;
                // Closing the socket cannot retract accepted daemon datagrams.
                std::process::exit(130);
            }
        }
    })
}

fn save(output: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.flush()) {
        drop(file);
        let _ = std::fs::remove_file(output);
        return Err(error.into());
    }
    Ok(())
}

async fn execute(command: Command, verbosity: super::Verbosity) -> Result<()> {
    let status = verbosity != super::Verbosity::Quiet;
    match command {
        Command::Send {
            file,
            kiss,
            repeat,
            interval_ms,
        } => {
            let mut bytes = Vec::new();
            std::fs::File::open(file)?
                .take(1_048_577)
                .read_to_end(&mut bytes)?;
            let packets = encode_file(&bytes)?;
            let mut socket = TcpStream::connect(kiss).await?;
            if status {
                eprintln!(
                    "Submitting {} bytes in {} chunk{} per copy to KISS.",
                    bytes.len(),
                    packets.len(),
                    if packets.len() == 1 { "" } else { "s" }
                );
            }
            let mut last: Option<tokio::time::Instant> = None;
            for _ in 0..repeat {
                for packet in &packets {
                    if let Some(last) = last {
                        let next = last
                            .checked_add(Duration::from_millis(interval_ms))
                            .context("broadcast interval exceeds clock range")?;
                        tokio::time::sleep_until(next).await;
                    }
                    socket.write_all(&kiss_encode_data(packet)).await?;
                    last = Some(tokio::time::Instant::now());
                }
            }
            if status {
                eprintln!("Submitted {repeat} copies to KISS; delivery is unconfirmed.");
            }
        }
        Command::Receive {
            kiss,
            output,
            idle_timeout,
        } => {
            if output.try_exists()? {
                bail!("broadcast output already exists: {}", output.display());
            }
            let mut socket = TcpStream::connect(kiss).await?;
            if status {
                eprintln!("Waiting for broadcast file chunks.");
            }
            let mut decoder = KissDecoder::new();
            let mut receiver = FileReceiver::new();
            let idle = Duration::from_secs(idle_timeout);
            let mut deadline = tokio::time::Instant::now()
                .checked_add(idle)
                .context("broadcast timeout exceeds clock range")?;
            let mut buffer = [0; 4096];
            loop {
                let n = match tokio::time::timeout_at(deadline, socket.read(&mut buffer)).await {
                    Ok(n) => n?,
                    Err(_) => {
                        bail!("broadcast incomplete: no new chunks for {idle_timeout} seconds")
                    }
                };
                if n == 0 {
                    bail!("broadcast KISS connection closed before completion");
                }
                for frame in decoder.push(&buffer[..n]) {
                    if let KissFrame::Data(packet) = frame {
                        let before = receiver.accepted_chunks();
                        match receiver.push(&packet) {
                            Ok(Some(bytes)) => {
                                save(&output, &bytes)?;
                                if status {
                                    eprintln!(
                                        "Received {} bytes into {}; checksum verified.",
                                        bytes.len(),
                                        output.display()
                                    );
                                }
                                return Ok(());
                            }
                            Ok(None) => {}
                            Err(error) => {
                                if verbosity.is_verbose() {
                                    eprintln!("Discarded broadcast packet: {error}");
                                }
                            }
                        }
                        if receiver.accepted_chunks() != before {
                            deadline = tokio::time::Instant::now()
                                .checked_add(idle)
                                .context("broadcast timeout exceeds clock range")?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn submissions_are_paced_and_identical() {
        let path = std::env::temp_dir().join(format!("coppa-pace-{}", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let command = Command::Send {
            file: path.clone(),
            kiss: listener.local_addr().unwrap().to_string(),
            repeat: 3,
            interval_ms: 30000,
        };
        let task = tokio::spawn(execute(command, super::super::Verbosity::Quiet));
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut decoder = KissDecoder::new();
        let mut buffer = [0; 4096];
        let mut times = Vec::new();
        let mut packets = Vec::new();
        while packets.len() < 3 {
            let n = socket.read(&mut buffer).await.unwrap();
            for frame in decoder.push(&buffer[..n]) {
                times.push(tokio::time::Instant::now());
                packets.push(frame);
            }
        }
        assert!(times
            .windows(2)
            .all(|t| t[1] - t[0] >= Duration::from_secs(30)));
        for packet in packets {
            assert!(matches!(packet, KissFrame::Data(p) if p == encode_file(b"hello").unwrap()[0]));
        }
        task.await.unwrap().unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
