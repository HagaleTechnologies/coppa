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

/// Whether either station's command port reports that the connected session
/// ended. Reads at most one line from each port per call. The answerer's
/// `DISCONNECTED` counts only after its own `CONNECTED` line: lines before
/// that belong to a handshake attempt that ended before this session (coppad's
/// ARDOP interface answers a cancelled inbound attempt with `DISCONNECTED`).
pub fn session_ended(
    caller: &mut Host,
    answerer: &mut Host,
    answerer_connected: &mut bool,
) -> Result<bool> {
    if caller
        .poll_line()?
        .is_some_and(|line| line.starts_with("DISCONNECTED"))
    {
        return Ok(true);
    }
    match answerer.poll_line()? {
        Some(line) if line.starts_with("CONNECTED") => *answerer_connected = true,
        Some(line) if line.starts_with("DISCONNECTED") => return Ok(*answerer_connected),
        _ => {}
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    /// A host joined to a fake TNC that has written `script` on its command
    /// port. The returned streams keep both connections open.
    fn station(script: &[u8]) -> (Host, [TcpStream; 2]) {
        let cmd = TcpListener::bind("127.0.0.1:0").unwrap();
        let data = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = Host::connect(
            &cmd.local_addr().unwrap().to_string(),
            &data.local_addr().unwrap().to_string(),
        )
        .unwrap();
        let (mut c, _) = cmd.accept().unwrap();
        let (d, _) = data.accept().unwrap();
        c.write_all(script).unwrap();
        (host, [c, d])
    }

    fn ended_within(
        caller: &mut Host,
        answerer: &mut Host,
        answerer_connected: &mut bool,
        window: Duration,
    ) -> bool {
        let deadline = Instant::now() + window;
        while Instant::now() < deadline {
            if session_ended(caller, answerer, answerer_connected).unwrap() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }

    #[test]
    fn caller_disconnect_ends_the_session() {
        let (mut a, _a) = station(b"DISCONNECTED\r");
        let (mut b, _b) = station(b"");
        let mut connected = false;
        assert!(ended_within(
            &mut a,
            &mut b,
            &mut connected,
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn answerer_disconnect_ends_the_session() {
        let (mut a, _a) = station(b"");
        let (mut b, _b) = station(b"CONNECTED N0AAA N0BBB 2300\rDISCONNECTED\r");
        let mut connected = false;
        assert!(ended_within(
            &mut a,
            &mut b,
            &mut connected,
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn answerer_disconnect_before_its_connected_is_ignored() {
        let (mut a, _a) = station(b"");
        let (mut b, _b) = station(
            b"TARGET N0BBB\rNEWSTATE IRS\rDISCONNECTED\rNEWSTATE DISC\rCONNECTED N0AAA 500MAX\rNEWSTATE IRS\r",
        );
        let mut connected = false;
        assert!(!ended_within(
            &mut a,
            &mut b,
            &mut connected,
            Duration::from_millis(500)
        ));
        assert!(connected, "the answerer's CONNECTED line was never read");
    }
}
