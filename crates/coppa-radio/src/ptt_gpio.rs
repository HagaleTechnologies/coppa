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
/// permissions setup this assumes. Unexports the pin on drop.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Loopback mock: records every GPIO level write instead of touching
    /// real hardware.
    #[derive(Clone, Default)]
    struct MockGpio {
        writes: Arc<Mutex<Vec<bool>>>,
    }

    impl GpioWriter for MockGpio {
        fn write_value(&mut self, high: bool) -> Result<()> {
            self.writes.lock().unwrap().push(high);
            Ok(())
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

    #[cfg(target_os = "linux")]
    #[test]
    fn test_gpio_ptt_open_unwritable_pin_errors() {
        // Pin 999999 won't be exportable without root/hardware support;
        // opening it should surface a clear error, not panic.
        let result = GpioPtt::open(999_999, false);
        assert!(result.is_err(), "opening an invalid GPIO pin should fail");
    }
}
