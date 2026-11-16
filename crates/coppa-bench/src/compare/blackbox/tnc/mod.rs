//! Host-side protocol adapters used by the black-box harness.

pub mod ardop;
pub mod kiss;
pub mod vara;

use anyhow::{bail, Result};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub struct Host {
    command: TcpStream,
    data: TcpStream,
    pending: Vec<u8>,
}

impl Host {
    pub fn connect(command: &str, data: &str) -> Result<Self> {
        let cmd = TcpStream::connect(command)?;
        let dat = TcpStream::connect(data)?;
        cmd.set_read_timeout(Some(Duration::from_millis(100)))?;
        dat.set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(Self {
            command: cmd,
            data: dat,
            pending: Vec::new(),
        })
    }

    pub fn command(&mut self, line: &str) -> Result<()> {
        self.command.write_all(line.as_bytes())?;
        self.command.write_all(b"\r")?;
        Ok(())
    }

    pub fn next_line(&mut self, deadline: Instant) -> Result<Option<String>> {
        loop {
            if let Some(pos) = self.pending.iter().position(|&b| b == b'\r' || b == b'\n') {
                let bytes: Vec<_> = self.pending.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&bytes[..pos]).trim().to_string();
                if !text.is_empty() {
                    return Ok(Some(text));
                }
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut b = [0u8; 256];
            match self.command.read(&mut b) {
                Ok(0) => bail!("TNC command connection closed"),
                Ok(n) => self.pending.extend_from_slice(&b[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub fn wait_for(&mut self, prefix: &str, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        while let Some(line) = self.next_line(deadline)? {
            if line.starts_with(prefix) {
                return Ok(line);
            }
            if line == "WRONG" || line.starts_with("FAULT") {
                bail!("TNC refused: {line}");
            }
        }
        bail!("TNC timed out waiting for {prefix}")
    }

    pub fn send_data(&mut self, bytes: &[u8]) -> Result<()> {
        self.data.write_all(bytes)?;
        Ok(())
    }

    pub fn receive_data(&mut self, bytes: &mut [u8]) -> Result<usize> {
        match self.data.read(bytes) {
            Ok(0) => bail!("TNC data connection closed"),
            Ok(n) => Ok(n),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(0)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn poll_line(&mut self) -> Result<Option<String>> {
        self.command.set_nonblocking(true)?;
        let mut bytes = [0u8; 256];
        let result = self.command.read(&mut bytes);
        self.command.set_nonblocking(false)?;
        match result {
            Ok(0) => return Ok(Some("DISCONNECTED".to_string())),
            Ok(n) => self.pending.extend_from_slice(&bytes[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        if let Some(pos) = self.pending.iter().position(|&b| b == b'\r' || b == b'\n') {
            let line: Vec<_> = self.pending.drain(..=pos).collect();
            return Ok(Some(
                String::from_utf8_lossy(&line[..pos]).trim().to_string(),
            ));
        }
        Ok(None)
    }
}
