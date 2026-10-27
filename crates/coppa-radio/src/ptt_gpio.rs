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
//! The pin number coppad takes is the kernel's sysfs GPIO number, not the BCM
//! pin: the GPIO chip's base plus the BCM pin. On Raspberry Pi OS with kernel
//! 6.6 or later the Pi 4's GPIO chip (label `pinctrl-bcm2711`) is registered
//! at base 512, so BCM GPIO 17 is sysfs GPIO 529. The base is assigned by the
//! kernel, so read it rather than assuming it:
//!
//! ```text
//! grep -H . /sys/class/gpio/gpiochip*/label /sys/class/gpio/gpiochip*/base
//! ```
//!
//! Then configure coppad with `ptt_method = "gpio:529"` (for BCM 17).
//!
//! `GpioPtt::open` exports the pin if it isn't already exported and sets its
//! direction to `out`. The process running `coppad` needs write access to
//! `/sys/class/gpio/export` and `/sys/class/gpio/gpioN/{direction,value}`: on
//! Raspberry Pi OS that means membership of the `gpio` group, not root.
//! Raspberry Pi OS's udev rule (`99-com.rules`) grants the group access to a
//! newly exported pin asynchronously, a few milliseconds after the export, so
//! an unprivileged process that writes `direction` at once can be refused.
//! `SysfsGpio::open` therefore re-tries a freshly exported pin for up to 2 s
//! while it is refused with `PermissionDenied`.
//!
//! The sysfs GPIO interface is deprecated upstream in favour of the GPIO
//! character device, but is still built into Raspberry Pi kernels
//! (`CONFIG_GPIO_SYSFS=y`). Test PTT on your image before relying on it.
//!
//! Wire the GPIO pin to your radio's PTT input through a transistor or relay
//! (a GPIO pin cannot directly switch a rig's PTT line/current).

use crate::{PttControl, PttState};
use anyhow::Result;
use std::io;
use std::time::Duration;

/// How many times `SysfsGpio::open` tries a freshly exported pin while udev's
/// 99-com.rules grants the gpio group access (40 x 50 ms = 2 s in total).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const EXPORT_SETTLE_ATTEMPTS: u32 = 40;
/// How long `SysfsGpio::open` waits between those tries.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const EXPORT_SETTLE_INTERVAL: Duration = Duration::from_millis(50);

/// Run `op` at least once and at most `max(attempts, 1)` times, sleeping
/// `interval` between tries, but only while it fails with
/// [`io::ErrorKind::PermissionDenied`]. A success or any other error returns
/// at once, so a wrong pin number (EINVAL) is reported immediately.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn retry_permission_denied<T>(
    attempts: u32,
    interval: Duration,
    mut op: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let attempts = attempts.max(1);
    let mut tried = 1;
    loop {
        match op() {
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && tried < attempts => {
                tried += 1;
                std::thread::sleep(interval);
            }
            result => return result,
        }
    }
}

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
    ///
    /// When this call exports the pin, the `direction` write and the `value`
    /// open are re-tried for up to 2 s while they are refused with
    /// `PermissionDenied`, covering the window before udev grants the `gpio`
    /// group access to the new pin. The export write itself is not re-tried,
    /// so a process outside the `gpio` group still fails at once. On failure
    /// the pin is left exported.
    pub fn open(pin: u32) -> Result<Self> {
        use anyhow::Context;

        let gpio_dir = std::path::PathBuf::from(format!("/sys/class/gpio/gpio{pin}"));
        let mut exported_now = false;
        if !gpio_dir.exists() {
            std::fs::write("/sys/class/gpio/export", pin.to_string())
                .with_context(|| format!("failed to export GPIO pin {pin}"))?;
            exported_now = true;
        }
        let attempts = if exported_now {
            EXPORT_SETTLE_ATTEMPTS
        } else {
            1
        };
        let direction_path = gpio_dir.join("direction");
        retry_permission_denied(attempts, EXPORT_SETTLE_INTERVAL, || {
            std::fs::write(&direction_path, "out")
        })
        .with_context(|| format!("failed to set GPIO pin {pin} direction to \"out\""))?;
        // Open `value` without writing a level (an inverted rig would key), so
        // the first real write from `EventLoop::new` cannot land in the same
        // udev window.
        let value_path = gpio_dir.join("value");
        retry_permission_denied(attempts, EXPORT_SETTLE_INTERVAL, || {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&value_path)
                .map(drop)
        })
        .with_context(|| format!("GPIO pin {pin} value is not writable"))?;
        Ok(Self { pin, value_path })
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

    #[test]
    fn settle_retry_returns_once_permission_is_granted() {
        let mut calls = 0;
        let result = retry_permission_denied(5, Duration::ZERO, || {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                Ok(7)
            }
        });
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls, 3);
    }

    #[test]
    fn settle_retry_gives_up_after_its_attempts() {
        // 4 attempts, all denied: the PermissionDenied is returned, not swallowed.
        let mut calls = 0;
        let result: io::Result<()> = retry_permission_denied(4, Duration::ZERO, || {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(calls, 4);
    }

    #[test]
    fn settle_retry_does_not_retry_other_errors() {
        // A wrong pin number is EINVAL: report it after exactly one call.
        let mut calls = 0;
        let result: io::Result<()> = retry_permission_denied(40, Duration::ZERO, || {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::InvalidInput))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(calls, 1);
    }

    #[test]
    fn settle_retry_tries_once_when_given_zero_attempts() {
        let mut calls = 0;
        let result: io::Result<()> = retry_permission_denied(0, Duration::ZERO, || {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

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
