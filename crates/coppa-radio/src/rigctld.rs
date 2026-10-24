//! TCP client for hamlib rigctld protocol.
//!
//! Connects to rigctld on localhost:4532 and implements
//! RadioControl + PttControl via the text-based protocol.

use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpStream};

use crate::{PttControl, PttState, RadioControl, RadioMode};

/// rigctld client for CAT control via hamlib.
pub struct RigctldClient {
    addr: String,
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    connected: bool,
    frequency: u64,
    mode: RadioMode,
    ptt: PttState,
}

impl RigctldClient {
    /// Connect to rigctld at the given address (e.g., "127.0.0.1:4532").
    pub fn connect(addr: &str) -> Result<Self> {
        let stream = TcpStream::connect(addr)
            .map_err(|e| anyhow!("Failed to connect to rigctld at {}: {}", addr, e))?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
        let reader = BufReader::new(stream.try_clone()?);

        Ok(Self {
            addr: addr.to_string(),
            stream,
            reader,
            connected: true,
            frequency: 14_074_000,
            mode: RadioMode::Usb,
            ptt: PttState::Rx,
        })
    }

    /// Ask the rig for its dial frequency (`f`), honestly: `Some(hz)` only on
    /// a clean numeric answer (which also refreshes the cache), `None` on any
    /// transport or parse failure. Unlike `RadioControl::get_frequency`, this
    /// never falls back to the cached or construction-default value, so a
    /// regulatory check can tell "the rig says X" from "nobody answered".
    ///
    /// Any failure invalidates the connection: a timed-out read can leave
    /// the late reply buffered on the old socket. The next command must
    /// reconnect successfully before sending or reading anything, even if
    /// earlier reconnect attempts failed.
    pub fn poll_frequency(&mut self) -> Option<u64> {
        let answer = self
            .command("f")
            .ok()
            .and_then(|resp| resp.trim().parse::<u64>().ok());
        match answer {
            Some(freq) => {
                self.frequency = freq;
                Some(freq)
            }
            None => {
                self.connected = false;
                let _ = self.stream.shutdown(Shutdown::Both);
                None
            }
        }
    }

    /// Replace the connection with a fresh one to the same address, dropping
    /// anything still buffered on the old socket. Bounded by a 2 s connect
    /// timeout per resolved address.
    fn reconnect(&mut self) -> Result<()> {
        use std::net::ToSocketAddrs;
        let timeout = std::time::Duration::from_secs(2);
        let mut last_err = anyhow!("rigctld address {} did not resolve", self.addr);
        for sock in self.addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&sock, timeout) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(timeout))?;
                    stream.set_write_timeout(Some(timeout))?;
                    self.reader = BufReader::new(stream.try_clone()?);
                    self.stream = stream;
                    self.connected = true;
                    return Ok(());
                }
                Err(e) => last_err = anyhow!("Failed to reconnect to rigctld at {}: {}", sock, e),
            }
        }
        Err(last_err)
    }

    /// Send a command and read one line of response.
    fn command(&mut self, cmd: &str) -> Result<String> {
        if !self.connected {
            self.reconnect()?;
        }
        writeln!(self.stream, "{}", cmd).map_err(|e| anyhow!("Failed to send command: {}", e))?;
        self.stream.flush()?;

        let mut response = String::new();
        self.reader
            .read_line(&mut response)
            .map_err(|e| anyhow!("Failed to read response: {}", e))?;
        Ok(response.trim().to_string())
    }

    /// Send a command and read two lines of response (e.g., mode + passband).
    fn command_two_lines(&mut self, cmd: &str) -> Result<(String, String)> {
        if !self.connected {
            self.reconnect()?;
        }
        writeln!(self.stream, "{}", cmd).map_err(|e| anyhow!("Failed to send command: {}", e))?;
        self.stream.flush()?;

        let mut line1 = String::new();
        self.reader
            .read_line(&mut line1)
            .map_err(|e| anyhow!("Failed to read response line 1: {}", e))?;
        let mut line2 = String::new();
        self.reader
            .read_line(&mut line2)
            .map_err(|e| anyhow!("Failed to read response line 2: {}", e))?;
        Ok((line1.trim().to_string(), line2.trim().to_string()))
    }

    fn parse_mode(s: &str) -> RadioMode {
        match s.to_uppercase().as_str() {
            "USB" => RadioMode::Usb,
            "LSB" => RadioMode::Lsb,
            "AM" => RadioMode::Am,
            "FM" => RadioMode::Fm,
            "PKTUSB" | "DIGU" | "DATA" => RadioMode::Digital,
            _ => RadioMode::Usb,
        }
    }

    fn mode_string(mode: RadioMode) -> &'static str {
        match mode {
            RadioMode::Usb => "USB",
            RadioMode::Lsb => "LSB",
            RadioMode::Am => "AM",
            RadioMode::Fm => "FM",
            RadioMode::Digital => "PKTUSB",
        }
    }
}

impl RadioControl for RigctldClient {
    fn get_frequency(&mut self) -> Result<u64> {
        match self.command("f") {
            Ok(resp) => {
                if let Ok(freq) = resp.trim().parse::<u64>() {
                    self.frequency = freq;
                    Ok(freq)
                } else {
                    Ok(self.frequency) // fallback to cached
                }
            }
            Err(_) => Ok(self.frequency), // fallback to cached on error
        }
    }

    fn set_frequency(&mut self, freq_hz: u64) -> Result<()> {
        let response = self.command(&format!("F {}", freq_hz))?;
        if response.starts_with("RPRT") {
            let code: i32 = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(-1);
            if code != 0 {
                return Err(anyhow!("rigctld set_frequency failed: {}", response));
            }
        }
        self.frequency = freq_hz;
        Ok(())
    }

    fn get_mode(&mut self) -> Result<RadioMode> {
        // rigctld 'm' command returns two lines: mode string and passband width
        match self.command_two_lines("m") {
            Ok((mode_str, _passband)) => {
                let mode = Self::parse_mode(mode_str.trim());
                self.mode = mode;
                Ok(mode)
            }
            Err(_) => Ok(self.mode), // fallback to cached on error
        }
    }

    fn set_mode(&mut self, mode: RadioMode) -> Result<()> {
        let mode_str = Self::mode_string(mode);
        let response = self.command(&format!("M {} 0", mode_str))?;
        if response.starts_with("RPRT") {
            let code: i32 = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(-1);
            if code != 0 {
                return Err(anyhow!("rigctld set_mode failed: {}", response));
            }
        }
        self.mode = mode;
        Ok(())
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        match self.command("t") {
            Ok(resp) => {
                let state = match resp.trim() {
                    "0" => PttState::Rx,
                    _ => PttState::Tx,
                };
                self.ptt = state;
                Ok(state)
            }
            Err(_) => Ok(self.ptt), // fallback to cached on error
        }
    }

    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        let val = match state {
            PttState::Rx => 0,
            PttState::Tx => 1,
        };
        let response = self.command(&format!("T {}", val))?;
        if response.starts_with("RPRT") {
            let code: i32 = response
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(-1);
            if code != 0 {
                return Err(anyhow!("rigctld set_ptt failed: {}", response));
            }
        }
        self.ptt = state;
        Ok(())
    }

    fn signal_strength(&mut self) -> Result<f32> {
        match self.command("l STRENGTH") {
            Ok(resp) => Ok(resp.trim().parse::<f32>().unwrap_or(0.0)),
            Err(_) => Ok(0.0),
        }
    }
}

impl PttControl for RigctldClient {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        RadioControl::set_ptt(self, state)
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        RadioControl::get_ptt(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_reconnect_cannot_reuse_a_late_frequency_reply() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let (sent_tx, sent_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Keep the existing connection alive but refuse reconnections.
            drop(listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            ready_tx.send(()).unwrap();
            let mut command = String::new();
            reader.read_line(&mut command).unwrap();
            assert_eq!(command.trim(), "f");
            reply_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            // The reply belonged to the timed-out query, before a retune.
            let _ = writeln!(stream, "144500000");
            sent_tx.send(()).unwrap();
            command.clear();
            if reader.read_line(&mut command).unwrap_or(0) > 0 {
                assert_eq!(command.trim(), "f");
                let _ = writeln!(stream, "14074000");
            }
        });
        let mut client = RigctldClient::connect(&addr).unwrap();
        client
            .stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(client.poll_frequency(), None, "the first query times out");
        reply_tx.send(()).unwrap();
        sent_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let next = client.poll_frequency();
        drop(client);
        server.join().unwrap();
        assert_eq!(
            next, None,
            "a failed reconnect must not reuse the old socket"
        );
    }

    #[test]
    fn test_parse_mode() {
        assert_eq!(RigctldClient::parse_mode("USB"), RadioMode::Usb);
        assert_eq!(RigctldClient::parse_mode("LSB"), RadioMode::Lsb);
        assert_eq!(RigctldClient::parse_mode("AM"), RadioMode::Am);
        assert_eq!(RigctldClient::parse_mode("FM"), RadioMode::Fm);
        assert_eq!(RigctldClient::parse_mode("PKTUSB"), RadioMode::Digital);
        assert_eq!(RigctldClient::parse_mode("DIGU"), RadioMode::Digital);
    }

    #[test]
    fn test_mode_string() {
        assert_eq!(RigctldClient::mode_string(RadioMode::Usb), "USB");
        assert_eq!(RigctldClient::mode_string(RadioMode::Lsb), "LSB");
        assert_eq!(RigctldClient::mode_string(RadioMode::Digital), "PKTUSB");
    }
}
