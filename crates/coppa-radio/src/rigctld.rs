//! TCP client for hamlib rigctld protocol.
//!
//! Connects to rigctld on localhost:4532 and implements
//! RadioControl + PttControl via the text-based protocol.

use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;

use crate::{PttControl, PttState, RadioControl, RadioMode};

/// rigctld client for CAT control via hamlib.
pub struct RigctldClient {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
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
            stream,
            reader,
            frequency: 14_074_000,
            mode: RadioMode::Usb,
            ptt: PttState::Rx,
        })
    }

    /// Send a command and read one line of response.
    fn command(&mut self, cmd: &str) -> Result<String> {
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
