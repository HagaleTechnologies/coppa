//! Serial port PTT control via DTR/RTS lines.

use crate::{PttControl, PttState};
use anyhow::Result;

/// Which serial control line to use for PTT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialPttLine {
    Dtr,
    Rts,
}

/// Serial port PTT using DTR or RTS control lines.
///
/// Note: Actual serial port access requires the `serialport` crate.
/// This implementation tracks state for the interface but actual hardware
/// control would need the platform serial port API.
pub struct SerialPtt {
    port_name: String,
    line: SerialPttLine,
    state: PttState,
    inverted: bool,
}

impl SerialPtt {
    /// Create a serial PTT controller.
    ///
    /// * `port_name` - Serial port path (e.g., "/dev/ttyUSB0", "COM3")
    /// * `line` - Which control line to use (DTR or RTS)
    /// * `inverted` - If true, PTT=TX drives the line LOW
    pub fn new(port_name: &str, line: SerialPttLine, inverted: bool) -> Result<Self> {
        Ok(Self {
            port_name: port_name.to_string(),
            line,
            state: PttState::Rx,
            inverted,
        })
    }

    /// Get the configured port name.
    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// Get the configured control line.
    pub fn line(&self) -> SerialPttLine {
        self.line
    }

    /// Whether the logic is inverted.
    pub fn is_inverted(&self) -> bool {
        self.inverted
    }
}

impl PttControl for SerialPtt {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        // STUB: requires the `serialport` crate and physical hardware.
        //
        // Real implementation would:
        //   1. Open the serial port: serialport::new(&self.port_name, 9600).open()
        //   2. Determine active-high/low based on self.inverted
        //   3. Match self.line:
        //      - SerialPttLine::Dtr => port.write_data_terminal_ready(active)
        //      - SerialPttLine::Rts => port.write_request_to_send(active)
        //   4. Keep port handle open for duration of TX
        self.state = state;
        Ok(())
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_ptt_create() {
        let ptt = SerialPtt::new("/dev/ttyUSB0", SerialPttLine::Dtr, false).unwrap();
        assert_eq!(ptt.port_name(), "/dev/ttyUSB0");
        assert_eq!(ptt.line(), SerialPttLine::Dtr);
        assert!(!ptt.is_inverted());
    }

    #[test]
    fn test_serial_ptt_roundtrip() {
        let mut ptt = SerialPtt::new("COM3", SerialPttLine::Rts, true).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
        ptt.set_ptt(PttState::Tx).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx);
    }
}
