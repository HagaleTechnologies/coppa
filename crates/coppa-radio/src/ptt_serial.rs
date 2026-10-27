//! Serial port PTT control via DTR/RTS lines.
//!
//! Feature-gated behind `serial-ptt` (pulls in the `serialport` crate),
//! following the same one-feature-per-hardware-backend pattern as
//! `coppa-audio`'s `cpal-backend`.

use crate::{PttControl, PttState};
use anyhow::{Context, Result};

/// Which serial control line to use for PTT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialPttLine {
    Dtr,
    Rts,
}

/// Abstraction over "toggle a serial port's modem-control lines."
///
/// `SerialPtt` is generic over this trait so its PTT state-machine logic
/// (which line, active-high/low, current state) can be exercised in unit
/// tests against a loopback mock, with no real serial hardware required. The
/// real `serialport` crate's `Box<dyn serialport::SerialPort>` implements it
/// below.
pub trait SerialLines: Send {
    fn write_data_terminal_ready(&mut self, level: bool) -> Result<()>;
    fn write_request_to_send(&mut self, level: bool) -> Result<()>;
}

impl SerialLines for Box<dyn serialport::SerialPort> {
    fn write_data_terminal_ready(&mut self, level: bool) -> Result<()> {
        serialport::SerialPort::write_data_terminal_ready(self.as_mut(), level)
            .context("failed to set DTR line")
    }

    fn write_request_to_send(&mut self, level: bool) -> Result<()> {
        serialport::SerialPort::write_request_to_send(self.as_mut(), level)
            .context("failed to set RTS line")
    }
}

/// Serial port PTT using DTR or RTS control lines.
///
/// Generic over `P: SerialLines` so it can be driven by a real open serial
/// port (`SerialPtt::open`, real hardware) or by a mock in tests
/// (`SerialPtt::with_port`).
///
/// Releases PTT on drop if keyed.
pub struct SerialPtt<P: SerialLines = Box<dyn serialport::SerialPort>> {
    port_name: String,
    line: SerialPttLine,
    state: PttState,
    inverted: bool,
    port: P,
}

impl<P: SerialLines> SerialPtt<P> {
    /// Construct from an already-open port (or a test mock). Does not touch
    /// hardware itself -- `port` is assumed already open/ready.
    ///
    /// * `port_name` - Serial port path (e.g., "/dev/ttyUSB0", "COM3"), kept
    ///   for diagnostics/`port_name()` only.
    /// * `line` - Which control line to use (DTR or RTS)
    /// * `inverted` - If true, PTT=TX drives the line LOW
    pub fn with_port(port_name: &str, line: SerialPttLine, inverted: bool, port: P) -> Self {
        Self {
            port_name: port_name.to_string(),
            line,
            state: PttState::Rx,
            inverted,
            port,
        }
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

impl SerialPtt<Box<dyn serialport::SerialPort>> {
    /// Open the real serial port at `port_name` and construct a PTT
    /// controller driving `line`.
    ///
    /// The baud rate is irrelevant for DTR/RTS-only PTT (no data is ever
    /// written/read on the port), so a conservative default is used.
    pub fn open(port_name: &str, line: SerialPttLine, inverted: bool) -> Result<Self> {
        let port = serialport::new(port_name, 9600)
            .timeout(std::time::Duration::from_millis(100))
            .open()
            .with_context(|| format!("failed to open serial port {port_name}"))?;
        Ok(Self::with_port(port_name, line, inverted, port))
    }
}

impl<P: SerialLines> PttControl for SerialPtt<P> {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        let active = matches!(state, PttState::Tx);
        let level = if self.inverted { !active } else { active };
        match self.line {
            SerialPttLine::Dtr => self.port.write_data_terminal_ready(level)?,
            SerialPttLine::Rts => self.port.write_request_to_send(level)?,
        }
        self.state = state;
        Ok(())
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        Ok(self.state)
    }
}

impl<P: SerialLines> Drop for SerialPtt<P> {
    /// COP-31: drive the PTT line to its RX level if this handle left it
    /// keyed (`state` only changes on a successful write).
    fn drop(&mut self) {
        if self.state == PttState::Tx {
            if let Err(e) = PttControl::set_ptt(self, PttState::Rx) {
                eprintln!(
                    "SerialPtt: failed to release PTT on {} on drop: {e}",
                    self.port_name
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PttBackend;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// Loopback mock: records every successful DTR/RTS level write instead
    /// of touching real hardware. Clones share the logs, so writes made while
    /// the owning `SerialPtt` drops stay observable.
    #[derive(Clone, Default)]
    struct MockPort {
        dtr_writes: Arc<Mutex<Vec<bool>>>,
        rts_writes: Arc<Mutex<Vec<bool>>>,
        /// When set, the next write (either line) fails and is not recorded.
        fail_next_write: Arc<AtomicBool>,
    }

    impl MockPort {
        fn fail_if_armed(&self) -> Result<()> {
            if self.fail_next_write.swap(false, Ordering::SeqCst) {
                anyhow::bail!("mock serial line write failure");
            }
            Ok(())
        }
    }

    impl SerialLines for MockPort {
        fn write_data_terminal_ready(&mut self, level: bool) -> Result<()> {
            self.fail_if_armed()?;
            self.dtr_writes.lock().unwrap().push(level);
            Ok(())
        }

        fn write_request_to_send(&mut self, level: bool) -> Result<()> {
            self.fail_if_armed()?;
            self.rts_writes.lock().unwrap().push(level);
            Ok(())
        }
    }

    /// (the PTT line's writes, the other line's writes)
    fn line_writes(mock: &MockPort, line: SerialPttLine) -> (Vec<bool>, Vec<bool>) {
        let dtr = mock.dtr_writes.lock().unwrap().clone();
        let rts = mock.rts_writes.lock().unwrap().clone();
        match line {
            SerialPttLine::Dtr => (dtr, rts),
            SerialPttLine::Rts => (rts, dtr),
        }
    }

    const LINES: [SerialPttLine; 2] = [SerialPttLine::Dtr, SerialPttLine::Rts];

    #[test]
    fn test_serial_ptt_create() {
        let mock = MockPort::default();
        let ptt = SerialPtt::with_port("/dev/ttyUSB0", SerialPttLine::Dtr, false, mock);
        assert_eq!(ptt.port_name(), "/dev/ttyUSB0");
        assert_eq!(ptt.line(), SerialPttLine::Dtr);
        assert!(!ptt.is_inverted());
    }

    #[test]
    fn test_serial_ptt_roundtrip() {
        let mock = MockPort::default();
        let mut ptt = SerialPtt::with_port("COM3", SerialPttLine::Rts, true, mock);
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
        ptt.set_ptt(PttState::Tx).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx);
    }

    #[test]
    fn test_serial_ptt_dtr_sets_and_clears() {
        let mock = MockPort::default();
        let mut ptt = SerialPtt::with_port("/dev/ttyUSB0", SerialPttLine::Dtr, false, mock.clone());

        ptt.set_ptt(PttState::Tx).unwrap();
        ptt.set_ptt(PttState::Rx).unwrap();

        assert_eq!(*mock.dtr_writes.lock().unwrap(), vec![true, false]);
        assert!(
            mock.rts_writes.lock().unwrap().is_empty(),
            "DTR-line PTT must not touch RTS"
        );
    }

    #[test]
    fn test_serial_ptt_rts_sets_and_clears() {
        let mock = MockPort::default();
        let mut ptt = SerialPtt::with_port("/dev/ttyUSB0", SerialPttLine::Rts, false, mock.clone());

        ptt.set_ptt(PttState::Tx).unwrap();
        ptt.set_ptt(PttState::Rx).unwrap();

        assert_eq!(*mock.rts_writes.lock().unwrap(), vec![true, false]);
        assert!(
            mock.dtr_writes.lock().unwrap().is_empty(),
            "RTS-line PTT must not touch DTR"
        );
    }

    #[test]
    fn test_serial_ptt_inverted_drives_line_low_on_tx() {
        let mock = MockPort::default();
        let mut ptt = SerialPtt::with_port("/dev/ttyUSB0", SerialPttLine::Dtr, true, mock.clone());

        ptt.set_ptt(PttState::Tx).unwrap();
        ptt.set_ptt(PttState::Rx).unwrap();

        assert_eq!(
            *mock.dtr_writes.lock().unwrap(),
            vec![false, true],
            "inverted PTT should drive the line LOW on TX and HIGH on RX"
        );
    }

    #[test]
    fn dropping_a_keyed_serial_ptt_drives_the_line_to_rx() {
        for line in LINES {
            for (inverted, expected) in [(false, [true, false]), (true, [false, true])] {
                let mock = MockPort::default();
                let mut ptt = SerialPtt::with_port("/dev/ttyUSB0", line, inverted, mock.clone());
                ptt.set_ptt(PttState::Tx).unwrap();
                drop(ptt);
                let (ptt_line, other) = line_writes(&mock, line);
                assert_eq!(ptt_line, expected, "{line:?}, inverted = {inverted}");
                assert!(other.is_empty(), "{line:?} must not touch the other line");
            }
        }
    }

    #[test]
    fn dropping_an_unkeyed_serial_ptt_writes_nothing() {
        for line in LINES {
            let mock = MockPort::default();
            drop(SerialPtt::with_port(
                "/dev/ttyUSB0",
                line,
                false,
                mock.clone(),
            ));
            assert_eq!(line_writes(&mock, line), (vec![], vec![]), "{line:?}");

            // Released explicitly before the drop: nothing more is written.
            let mock = MockPort::default();
            let mut ptt = SerialPtt::with_port("/dev/ttyUSB0", line, false, mock.clone());
            ptt.set_ptt(PttState::Tx).unwrap();
            ptt.set_ptt(PttState::Rx).unwrap();
            drop(ptt);
            assert_eq!(
                line_writes(&mock, line),
                (vec![true, false], vec![]),
                "{line:?}"
            );
        }
    }

    #[test]
    fn a_failed_serial_release_is_retried_on_drop() {
        for line in LINES {
            let mock = MockPort::default();
            let mut ptt = SerialPtt::with_port("/dev/ttyUSB0", line, false, mock.clone());
            ptt.set_ptt(PttState::Tx).unwrap();
            mock.fail_next_write.store(true, Ordering::SeqCst);
            assert!(ptt.set_ptt(PttState::Rx).is_err());
            assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx, "still keyed");
            drop(ptt);
            assert_eq!(
                line_writes(&mock, line),
                (vec![true, false], vec![]),
                "{line:?}"
            );
        }
    }

    #[test]
    fn dropping_a_boxed_serial_backend_releases_through_ptt_backend() {
        for line in LINES {
            let mock = MockPort::default();
            let ptt = SerialPtt::with_port("/dev/ttyUSB0", line, false, mock.clone());
            let mut backend = PttBackend::Other(Box::new(ptt));
            backend.set_ptt(PttState::Tx).unwrap();
            drop(backend);
            assert_eq!(
                line_writes(&mock, line),
                (vec![true, false], vec![]),
                "{line:?}"
            );
        }
    }

    #[test]
    fn test_serial_ptt_open_nonexistent_port_errors() {
        let result = SerialPtt::open(
            "/dev/coppa-nonexistent-test-port",
            SerialPttLine::Dtr,
            false,
        );
        assert!(
            result.is_err(),
            "opening a nonexistent serial port should fail"
        );
    }
}
