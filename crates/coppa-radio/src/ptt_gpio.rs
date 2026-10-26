//! GPIO PTT control via raw Linux sysfs (`/sys/class/gpio`) writes.
//!
//! No new crate dependency: this drives PTT by writing directly to the
//! kernel's sysfs GPIO interface, the way a shell script would
//! (`echo 1 > /sys/class/gpio/gpioN/value`). Real GPIO access
//! (`GpioPtt::open`/`SysfsGpio`) only exists on Linux (`target_os = "linux"`)
//! -- sysfs GPIO is a Linux kernel feature and has no equivalent elsewhere.
//! The rest of `GpioPtt`'s logic is OS-agnostic and generic over a
//! [`GpioWriter`] trait, so it can be unit-tested with a loopback mock on any
//! platform.
//!
//! # Raspberry Pi usage
//!
//! Export the BCM pin once (e.g. via a udev rule or `/etc/rc.local`) and set
//! its direction to `out`:
//!
//! ```text
//! echo 17 > /sys/class/gpio/export
//! echo out > /sys/class/gpio/gpio17/direction
//! ```
//!
//! `GpioPtt::open` does this automatically on first use if the pin isn't
//! already exported. Then configure coppad with `ptt = "gpio:17"`.
//!
//! The process running `coppad` needs write access to
//! `/sys/class/gpio/export` and `/sys/class/gpio/gpio17/{direction,value}` --
//! on stock Raspberry Pi OS this typically means adding the user to the
//! `gpio` group (`sudo usermod -aG gpio $USER`) rather than running as root.
//! Wire the GPIO pin to your radio's PTT input through a transistor or relay
//! (a GPIO pin cannot directly switch a rig's PTT line/current).

use crate::{PttControl, PttState};
use anyhow::Result;

/// Abstraction over "write a digital level to a GPIO line."
///
/// `GpioPtt` is generic over this trait so its PTT state-machine logic can be
/// exercised in unit tests against a loopback mock, with no real
/// `/sys/class/gpio` (or even Linux) required. The real sysfs-backed
/// implementation ([`SysfsGpio`]) is only available on Linux.
pub trait GpioWriter: Send {
    fn write_value(&mut self, high: bool) -> Result<()>;
}

/// Real sysfs-backed GPIO line: writes `"1"`/`"0"` to
/// `/sys/class/gpio/gpio<N>/value`. See the module docs for the export/
/// permissions setup this assumes. Unexports the pin on drop; `GpioPtt::drop`
/// lowers it first.
#[cfg(target_os = "linux")]
pub struct SysfsGpio {
    pin: u32,
    value_path: std::path::PathBuf,
}

#[cfg(target_os = "linux")]
impl SysfsGpio {
    /// Export (if not already exported) and configure GPIO pin `pin` for
    /// output.
    pub fn open(pin: u32) -> Result<Self> {
        use anyhow::Context;

        let gpio_dir = std::path::PathBuf::from(format!("/sys/class/gpio/gpio{pin}"));
        if !gpio_dir.exists() {
            std::fs::write("/sys/class/gpio/export", pin.to_string())
                .with_context(|| format!("failed to export GPIO pin {pin}"))?;
        }
        std::fs::write(gpio_dir.join("direction"), "out")
            .with_context(|| format!("failed to set GPIO pin {pin} direction to \"out\""))?;
        Ok(Self {
            pin,
            value_path: gpio_dir.join("value"),
        })
    }
}

#[cfg(target_os = "linux")]
impl GpioWriter for SysfsGpio {
    fn write_value(&mut self, high: bool) -> Result<()> {
        use anyhow::Context;
        std::fs::write(&self.value_path, if high { "1" } else { "0" })
            .with_context(|| format!("failed to write GPIO value at {:?}", self.value_path))
    }
}

#[cfg(target_os = "linux")]
impl Drop for SysfsGpio {
    fn drop(&mut self) {
        // Best-effort unexport; nothing useful to do if this fails (e.g. pin
        // was already unexported by another process).
        let _ = std::fs::write("/sys/class/gpio/unexport", self.pin.to_string());
    }
}

/// GPIO PTT: drives a single GPIO line high/low for TX/RX.
///
/// Generic over `W: GpioWriter` so it can be driven by a real exported pin
/// (`GpioPtt::open`, Linux only) or by a mock in tests (`GpioPtt::with_writer`).
///
/// Releases PTT on drop if keyed.
pub struct GpioPtt<W: GpioWriter> {
    pin: u32,
    inverted: bool,
    state: PttState,
    writer: W,
}

impl<W: GpioWriter> GpioPtt<W> {
    /// Construct from an already-configured writer (real or mock).
    ///
    /// * `pin` - GPIO pin number, kept for diagnostics/`pin()` only.
    /// * `inverted` - If true, PTT=TX drives the line LOW.
    pub fn with_writer(pin: u32, inverted: bool, writer: W) -> Self {
        Self {
            pin,
            inverted,
            state: PttState::Rx,
            writer,
        }
    }

    /// Get the configured pin number.
    pub fn pin(&self) -> u32 {
        self.pin
    }

    /// Whether the logic is inverted.
    pub fn is_inverted(&self) -> bool {
        self.inverted
    }
}

#[cfg(target_os = "linux")]
impl GpioPtt<SysfsGpio> {
    /// Export and open a real GPIO pin via sysfs.
    pub fn open(pin: u32, inverted: bool) -> Result<Self> {
        let writer = SysfsGpio::open(pin)?;
        Ok(Self::with_writer(pin, inverted, writer))
    }
}

impl<W: GpioWriter> PttControl for GpioPtt<W> {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        let active = matches!(state, PttState::Tx);
        let high = if self.inverted { !active } else { active };
        self.writer.write_value(high)?;
        self.state = state;
        Ok(())
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        Ok(self.state)
    }
}

impl<W: GpioWriter> Drop for GpioPtt<W> {
    /// COP-31: drive the PTT line to its RX level if this handle left it
    /// keyed (`state` only changes on a successful write).
    fn drop(&mut self) {
        // This runs before the writer's own `Drop` (fields drop after the
        // containing struct's `drop`), so `SysfsGpio::drop` unexports an
        // already-lowered pin.
        if self.state == PttState::Tx {
            if let Err(e) = PttControl::set_ptt(self, PttState::Rx) {
                eprintln!(
                    "GpioPtt: failed to release PTT on GPIO pin {} on drop: {e}",
                    self.pin
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

    /// Loopback mock: records every successful GPIO level write instead of
    /// touching real hardware. Clones share the log, so writes made while the
    /// owning `GpioPtt` drops stay observable.
    #[derive(Clone, Default)]
    struct MockGpio {
        writes: Arc<Mutex<Vec<bool>>>,
        /// When set, the next write fails (and is not recorded).
        fail_next_write: Arc<AtomicBool>,
    }

    impl GpioWriter for MockGpio {
        fn write_value(&mut self, high: bool) -> Result<()> {
            if self.fail_next_write.swap(false, Ordering::SeqCst) {
                anyhow::bail!("mock GPIO write failure");
            }
            self.writes.lock().unwrap().push(high);
            Ok(())
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum GpioEvent {
        High,
        Low,
        Unexported,
    }

    /// Mock writer whose own `Drop` records an `Unexported` marker, standing
    /// in for `SysfsGpio`'s unexport-on-drop.
    struct UnexportingGpio {
        log: Arc<Mutex<Vec<GpioEvent>>>,
    }

    impl GpioWriter for UnexportingGpio {
        fn write_value(&mut self, high: bool) -> Result<()> {
            let event = if high {
                GpioEvent::High
            } else {
                GpioEvent::Low
            };
            self.log.lock().unwrap().push(event);
            Ok(())
        }
    }

    impl Drop for UnexportingGpio {
        fn drop(&mut self) {
            self.log.lock().unwrap().push(GpioEvent::Unexported);
        }
    }

    #[test]
    fn test_gpio_ptt_create() {
        let mock = MockGpio::default();
        let ptt = GpioPtt::with_writer(17, false, mock);
        assert_eq!(ptt.pin(), 17);
        assert!(!ptt.is_inverted());
    }

    #[test]
    fn test_gpio_ptt_sets_and_clears_line() {
        let mock = MockGpio::default();
        let mut ptt = GpioPtt::with_writer(17, false, mock.clone());

        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
        ptt.set_ptt(PttState::Tx).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx);
        ptt.set_ptt(PttState::Rx).unwrap();

        assert_eq!(*mock.writes.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn test_gpio_ptt_inverted_drives_line_low_on_tx() {
        let mock = MockGpio::default();
        let mut ptt = GpioPtt::with_writer(17, true, mock.clone());

        ptt.set_ptt(PttState::Tx).unwrap();
        ptt.set_ptt(PttState::Rx).unwrap();

        assert_eq!(
            *mock.writes.lock().unwrap(),
            vec![false, true],
            "inverted PTT should drive the line LOW on TX and HIGH on RX"
        );
    }

    #[test]
    fn dropping_a_keyed_gpio_ptt_drives_the_line_to_rx() {
        for (inverted, expected) in [(false, [true, false]), (true, [false, true])] {
            let mock = MockGpio::default();
            let mut ptt = GpioPtt::with_writer(17, inverted, mock.clone());
            ptt.set_ptt(PttState::Tx).unwrap();
            drop(ptt);
            assert_eq!(
                *mock.writes.lock().unwrap(),
                expected,
                "inverted = {inverted}"
            );
        }
    }

    #[test]
    fn dropping_an_unkeyed_gpio_ptt_writes_nothing() {
        let mock = MockGpio::default();
        drop(GpioPtt::with_writer(17, false, mock.clone()));
        assert!(mock.writes.lock().unwrap().is_empty());

        // Released explicitly before the drop: nothing more is written.
        let mock = MockGpio::default();
        let mut ptt = GpioPtt::with_writer(17, false, mock.clone());
        ptt.set_ptt(PttState::Tx).unwrap();
        ptt.set_ptt(PttState::Rx).unwrap();
        drop(ptt);
        assert_eq!(*mock.writes.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn a_failed_gpio_release_is_retried_on_drop() {
        let mock = MockGpio::default();
        let mut ptt = GpioPtt::with_writer(17, false, mock.clone());
        ptt.set_ptt(PttState::Tx).unwrap();
        mock.fail_next_write.store(true, Ordering::SeqCst);
        assert!(ptt.set_ptt(PttState::Rx).is_err());
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx, "still keyed");
        drop(ptt);
        assert_eq!(*mock.writes.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn gpio_release_runs_before_the_writer_is_dropped() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let writer = UnexportingGpio { log: log.clone() };
        let mut ptt = GpioPtt::with_writer(17, false, writer);
        ptt.set_ptt(PttState::Tx).unwrap();
        drop(ptt);
        assert_eq!(
            *log.lock().unwrap(),
            vec![GpioEvent::High, GpioEvent::Low, GpioEvent::Unexported]
        );
    }

    #[test]
    fn dropping_a_boxed_backend_releases_through_ptt_backend() {
        let mock = MockGpio::default();
        let mut backend =
            PttBackend::Other(Box::new(GpioPtt::with_writer(17, false, mock.clone())));
        backend.set_ptt(PttState::Tx).unwrap();
        drop(backend);
        assert_eq!(*mock.writes.lock().unwrap(), vec![true, false]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gpio_ptt_open_unwritable_pin_errors() {
        // Pin 999999 won't be exportable without root/hardware support;
        // opening it should surface a clear error, not panic.
        let result = GpioPtt::open(999_999, false);
        assert!(result.is_err(), "opening an invalid GPIO pin should fail");
    }
}
