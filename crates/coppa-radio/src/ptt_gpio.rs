//! GPIO PTT control on Linux, through the GPIO character device
//! (`/dev/gpiochipN`) or the deprecated sysfs interface (`/sys/class/gpio`).
//!
//! Real GPIO access ([`CdevGpio`], [`SysfsGpio`]) only exists on Linux
//! (`target_os = "linux"`). The rest of `GpioPtt`'s logic, and the chip
//! selection behind the character-device form, are OS-agnostic: `GpioPtt` is
//! generic over a [`GpioWriter`] trait, so it can be unit-tested with a
//! loopback mock on any platform.
//!
//! # The character device (`gpio:<chip>:<line>`)
//!
//! COP-85: this is the form to use. coppad names a chip and a line offset on
//! it. The chip is matched against, in order, a path (`/dev/gpiochip0`), a
//! chip name (`gpiochip0`) or a chip label (`pinctrl-bcm2711`). A label that
//! two chips share is refused, since the kernel may number them either way.
//! On a Raspberry Pi's header chip the line offset is the BCM GPIO number, so
//! BCM GPIO 17 on a Pi 4 is `gpio:pinctrl-bcm2711:17`. List the chips with
//! `gpiodetect` and the lines with `gpioinfo` (Debian package `gpiod`).
//!
//! [`CdevGpio::open`] requests the line as an output at its RX level, so the
//! rig is not keyed by opening it. A line that another process holds (for
//! example a guardian from the previous run) is re-tried for up to 2 s; after
//! that the error names the holder's consumer label. The process running
//! `coppad` needs read/write access to `/dev/gpiochipN`: on Raspberry Pi OS,
//! membership of the `gpio` group.
//!
//! When the request is dropped the kernel frees the line, and its level is
//! then up to the GPIO driver. The daemon drives the line to RX before it lets
//! go; after a crash while keyed, the COP-31 guardian requests the line at its
//! RX level and then frees it again.
//!
//! # The deprecated sysfs interface (`gpio:<number>`)
//!
//! The old form takes the kernel's sysfs GPIO number, not the BCM pin: the
//! GPIO chip's base plus the BCM pin. On Raspberry Pi OS with kernel 6.6 or
//! later the Pi 4's GPIO chip (label `pinctrl-bcm2711`) is registered at base
//! 512, so BCM GPIO 17 is sysfs GPIO 529. [`sysfs_number_to_chardev`] maps a
//! sysfs number to the equivalent character-device chip and line.
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
//! character device. Raspberry Pi kernels still build it in
//! (`CONFIG_GPIO_SYSFS=y`), but other kernels may not.
//!
//! Wire the GPIO pin to your radio's PTT input through a transistor or relay
//! (a GPIO pin cannot directly switch a rig's PTT line/current).

use crate::{PttControl, PttState};
use anyhow::Result;
use std::io;
use std::path::PathBuf;
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
    op: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    retry_on(io::ErrorKind::PermissionDenied, attempts, interval, op)
}

/// Run `op` at least once and at most `max(attempts, 1)` times, sleeping
/// `interval` between tries, but only while it fails with an error of kind
/// `kind`. A success or any other error returns at once.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn retry_on<T>(
    kind: io::ErrorKind,
    attempts: u32,
    interval: Duration,
    mut op: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let attempts = attempts.max(1);
    let mut tried = 1;
    loop {
        match op() {
            Err(e) if e.kind() == kind && tried < attempts => {
                tried += 1;
                std::thread::sleep(interval);
            }
            result => return result,
        }
    }
}

/// One GPIO character device, as `gpiodetect` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipSummary {
    /// The device path, e.g. `/dev/gpiochip0`.
    pub path: PathBuf,
    /// The kernel's name for the chip, e.g. `gpiochip0`.
    pub name: String,
    /// The driver's label, e.g. `pinctrl-bcm2711`.
    pub label: String,
    /// How many lines the chip has.
    pub num_lines: u32,
}

impl ChipSummary {
    /// The label when there is one, else the name.
    fn display_name(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }
}

/// Pick the chip `selector` names from `chips`: a path (`/dev/gpiochip0`), a
/// chip name (`gpiochip0`) or a label (`pinctrl-bcm2711`), tried in that
/// order. A label that more than one chip carries is refused, and an error
/// for no match lists every chip there is.
pub fn select_chip<'a>(selector: &str, chips: &'a [ChipSummary]) -> Result<&'a ChipSummary> {
    if let Some(chip) = chips
        .iter()
        .find(|c| c.path.as_os_str() == selector || c.name == selector)
    {
        return Ok(chip);
    }
    let labelled: Vec<&ChipSummary> = chips.iter().filter(|c| c.label == selector).collect();
    match labelled.as_slice() {
        [chip] => Ok(chip),
        [] if chips.is_empty() => anyhow::bail!(
            "no GPIO chip matches {selector:?}; this system has no GPIO character devices \
             (/dev/gpiochip*)"
        ),
        [] => {
            let listed: Vec<String> = chips
                .iter()
                .map(|c| format!("{} ({}, {} lines)", c.name, c.label, c.num_lines))
                .collect();
            anyhow::bail!(
                "no GPIO chip matches {selector:?}; this system has {}",
                listed.join(", ")
            )
        }
        several => {
            let names: Vec<&str> = several.iter().map(|c| c.name.as_str()).collect();
            anyhow::bail!(
                "GPIO chip label {selector:?} is ambiguous: {} all carry it; name the chip \
                 (for example {:?}) instead",
                names.join(", "),
                names[0]
            )
        }
    }
}

/// Fail unless `line` is an offset on `chip`.
#[cfg_attr(not(all(target_os = "linux", feature = "gpio-ptt")), allow(dead_code))]
fn check_line_offset(chip: &ChipSummary, line: u32) -> Result<()> {
    if line < chip.num_lines {
        return Ok(());
    }
    match chip.num_lines {
        0 => anyhow::bail!("{} has no lines", chip.display_name()),
        n => anyhow::bail!("{} has {n} lines (0 to {})", chip.display_name(), n - 1),
    }
}

/// [`select_chip`] over the chips that could be read. When it fails and
/// some chip could not be read, the failure says so, since that chip may be
/// the one `selector` names.
#[cfg_attr(not(all(target_os = "linux", feature = "gpio-ptt")), allow(dead_code))]
fn select_readable_chip<'a>(
    selector: &str,
    chips: &'a [ChipSummary],
    unreadable: Option<&str>,
) -> Result<&'a ChipSummary> {
    match (select_chip(selector, chips), unreadable) {
        (Ok(chip), _) => Ok(chip),
        (Err(_), Some(read_err)) if chips.is_empty() => anyhow::bail!("{read_err}"),
        (Err(e), Some(read_err)) => anyhow::bail!("{e}; {read_err}"),
        (Err(e), None) => Err(e),
    }
}

/// Map a sysfs GPIO number to the character-device chip label and line
/// offset that drive the same pin. `chips` holds each sysfs chip's
/// `(base, ngpio, label)`, as [`read_sysfs_chips`] returns them. `None` when
/// no chip covers `number`, or the covering chip has no label or shares its
/// label with another chip (which [`select_chip`] would refuse).
pub fn sysfs_number_to_chardev(number: u32, chips: &[(u32, u32, String)]) -> Option<(String, u32)> {
    let (base, _, label) = chips
        .iter()
        .find(|(base, ngpio, _)| number >= *base && number - base < *ngpio)?;
    let unique = chips.iter().filter(|(_, _, l)| l == label).count() == 1;
    (!label.is_empty() && unique).then(|| (label.clone(), number - base))
}

/// Every sysfs GPIO chip's `(base, ngpio, label)`, read from
/// `/sys/class/gpio/gpiochip*`. Empty when the kernel has no sysfs GPIO.
#[cfg(target_os = "linux")]
pub fn read_sysfs_chips() -> Vec<(u32, u32, String)> {
    let Ok(entries) = std::fs::read_dir("/sys/class/gpio") else {
        return Vec::new();
    };
    let read = |dir: &std::path::Path, file: &str| {
        std::fs::read_to_string(dir.join(file)).map(|s| s.trim().to_string())
    };
    let mut chips = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("gpiochip") {
            continue;
        }
        let dir = entry.path();
        let (Ok(base), Ok(ngpio), Ok(label)) =
            (read(&dir, "base"), read(&dir, "ngpio"), read(&dir, "label"))
        else {
            continue;
        };
        if let (Ok(base), Ok(ngpio)) = (base.parse(), ngpio.parse()) {
            chips.push((base, ngpio, label));
        }
    }
    chips
}

/// Abstraction over "write a digital level to a GPIO line."
///
/// `GpioPtt` is generic over this trait so its PTT state-machine logic can be
/// exercised in unit tests against a loopback mock, with no real
/// `/sys/class/gpio` (or even Linux) required. The real sysfs-backed
/// implementation (`SysfsGpio`) is only available on Linux.
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

/// How many times `CdevGpio::open` requests a line another process holds
/// (40 x 50 ms = 2 s in total), covering a guardian from the previous run.
#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
const BUSY_ATTEMPTS: u32 = 40;
/// How long `CdevGpio::open` waits between those tries.
#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
const BUSY_INTERVAL: Duration = Duration::from_millis(50);

/// The OS error number inside a `gpiocdev` error, when there is one.
/// `Error::Os` carries it directly; `Error::Uapi` displays it through
/// `io::Error`, as `... (os error N)`.
#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
fn gpiocdev_errno(e: &gpiocdev::Error) -> Option<i32> {
    if let gpiocdev::Error::Os(errno) = e {
        return Some(errno.0);
    }
    let text = e.to_string();
    let start = text.rfind("(os error ")? + "(os error ".len();
    text[start..].split(')').next()?.parse().ok()
}

/// A `gpiocdev` error as an `io::Error` whose kind follows the OS error
/// number, so the retry helpers can match on it.
#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
fn gpiocdev_to_io(e: gpiocdev::Error) -> io::Error {
    let kind = gpiocdev_errno(&e)
        .map(|n| io::Error::from_raw_os_error(n).kind())
        .unwrap_or(io::ErrorKind::Other);
    io::Error::new(kind, e)
}

/// List every GPIO character device with its name, label and line count.
/// A chip that cannot be opened (for example one the user has no access to)
/// is left out of the list, and the first such failure is returned with it.
#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
fn list_chips() -> Result<(Vec<ChipSummary>, Option<String>)> {
    use anyhow::Context;
    let paths = gpiocdev::chip::chips()
        .map_err(gpiocdev_to_io)
        .context("failed to list the GPIO character devices in /dev")?;
    let mut chips = Vec::with_capacity(paths.len());
    let mut unreadable = None;
    for path in paths {
        match gpiocdev::Chip::from_path(&path).and_then(|chip| chip.info()) {
            Ok(info) => chips.push(ChipSummary {
                path,
                name: info.name,
                label: info.label,
                num_lines: info.num_lines,
            }),
            Err(e) => {
                unreadable.get_or_insert_with(|| {
                    format!(
                        "failed to read GPIO chip {}: {}",
                        path.display(),
                        gpiocdev_to_io(e)
                    )
                });
            }
        }
    }
    Ok((chips, unreadable))
}

/// Real GPIO character-device line (`/dev/gpiochipN`), held as a one-line
/// output request. See the module docs for chip selection and permissions.
/// Dropping it frees the line; `GpioPtt::drop` drives it to RX first.
#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
pub struct CdevGpio {
    request: gpiocdev::Request,
    line: u32,
}

#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
impl CdevGpio {
    /// Request line `line` on the chip `selector` names (a path, a chip name
    /// or a label) as an output starting at `idle_high`, labelled `consumer`.
    ///
    /// A line another process holds is re-tried for up to 2 s; the final
    /// error names the holder's consumer label.
    pub fn open(selector: &str, line: u32, idle_high: bool, consumer: &str) -> Result<Self> {
        use anyhow::Context;

        // `gpiocdev` lists chips by their canonical path, so a symlink such
        // as a compatibility /dev/gpiochipN link must be resolved to match.
        let resolved;
        let selector = if selector.starts_with('/') {
            resolved = std::fs::canonicalize(selector)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| selector.to_string());
            resolved.as_str()
        } else {
            selector
        };
        let (chips, unreadable) = list_chips()?;
        let chip = select_readable_chip(selector, &chips, unreadable.as_deref())?;
        check_line_offset(chip, line)?;
        let requested = retry_on(
            io::ErrorKind::ResourceBusy,
            BUSY_ATTEMPTS,
            BUSY_INTERVAL,
            || {
                gpiocdev::Request::builder()
                    .on_chip(&chip.path)
                    .with_consumer(consumer)
                    .with_line(line)
                    .as_output(gpiocdev::line::Value::from(idle_high))
                    .request()
                    .map_err(gpiocdev_to_io)
            },
        );
        match requested {
            Ok(request) => Ok(Self { request, line }),
            Err(e) if e.kind() == io::ErrorKind::ResourceBusy => {
                let holder = gpiocdev::Chip::from_path(&chip.path)
                    .and_then(|c| c.line_info(line))
                    .map(|info| info.consumer)
                    .unwrap_or_default();
                if holder.is_empty() {
                    anyhow::bail!("the line is in use ({e}), and its holder set no consumer label")
                }
                anyhow::bail!("the line is in use by {holder:?}")
            }
            Err(e) => Err(e).with_context(|| {
                format!("failed to request line {line} on {}", chip.path.display())
            }),
        }
    }
}

#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
impl GpioWriter for CdevGpio {
    fn write_value(&mut self, high: bool) -> Result<()> {
        use anyhow::Context;
        self.request
            .set_value(self.line, gpiocdev::line::Value::from(high))
            .map_err(gpiocdev_to_io)
            .with_context(|| format!("failed to set GPIO line {}", self.line))
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

#[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
impl GpioPtt<CdevGpio> {
    /// Request a real GPIO line through the character device, starting at
    /// its RX level. `selector` is a chip path, name or label; `consumer` is
    /// the label `gpioinfo` shows for the line while it is held.
    pub fn open_chardev(selector: &str, line: u32, inverted: bool, consumer: &str) -> Result<Self> {
        // RX drives the line low, or high when inverted.
        let writer = CdevGpio::open(selector, line, inverted, consumer)?;
        Ok(Self::with_writer(line, inverted, writer))
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
                tracing::error!(
                    pin = self.pin,
                    error = %e,
                    "GpioPtt: failed to release PTT on drop"
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

    #[test]
    fn retry_busy_returns_once_the_line_frees() {
        let mut calls = 0;
        let result = retry_on(io::ErrorKind::ResourceBusy, 40, Duration::ZERO, || {
            calls += 1;
            if calls < 3 {
                // EBUSY, as the kernel answers a request for a held line.
                Err(io::Error::from_raw_os_error(16))
            } else {
                Ok(5)
            }
        });
        assert_eq!(result.unwrap(), 5);
        assert_eq!(calls, 3);

        // Any other error returns at once.
        let mut calls = 0;
        let result: io::Result<()> =
            retry_on(io::ErrorKind::ResourceBusy, 40, Duration::ZERO, || {
                calls += 1;
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(calls, 1);
    }

    fn chip(n: u32, label: &str, num_lines: u32) -> ChipSummary {
        ChipSummary {
            path: PathBuf::from(format!("/dev/gpiochip{n}")),
            name: format!("gpiochip{n}"),
            label: label.to_string(),
            num_lines,
        }
    }

    fn pi4_chips() -> Vec<ChipSummary> {
        vec![
            chip(0, "pinctrl-bcm2711", 58),
            chip(1, "raspberrypi-exp-gpio", 8),
        ]
    }

    #[test]
    fn select_chip_by_path_name_or_label() {
        let chips = pi4_chips();
        for selector in ["/dev/gpiochip1", "gpiochip1", "raspberrypi-exp-gpio"] {
            assert_eq!(
                select_chip(selector, &chips).unwrap().name,
                "gpiochip1",
                "{selector}"
            );
        }
        assert_eq!(
            select_chip("pinctrl-bcm2711", &chips).unwrap().name,
            "gpiochip0"
        );
    }

    #[test]
    fn select_chip_refuses_an_ambiguous_label() {
        let chips = vec![chip(0, "gpio-mockup", 8), chip(3, "gpio-mockup", 8)];
        let err = select_chip("gpio-mockup", &chips).unwrap_err().to_string();
        assert!(err.contains("ambiguous"), "{err}");
        assert!(
            err.contains("gpiochip0") && err.contains("gpiochip3"),
            "{err}"
        );
        // The chip's own name still selects one of them.
        assert_eq!(select_chip("gpiochip3", &chips).unwrap().name, "gpiochip3");
    }

    #[test]
    fn select_chip_error_lists_the_chips() {
        let err = select_chip("pinctrl-rp1", &pi4_chips())
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "no GPIO chip matches \"pinctrl-rp1\"; this system has gpiochip0 \
             (pinctrl-bcm2711, 58 lines), gpiochip1 (raspberrypi-exp-gpio, 8 lines)"
        );
    }

    #[test]
    fn select_chip_error_with_no_chips_says_so() {
        let err = select_chip("pinctrl-bcm2711", &[]).unwrap_err().to_string();
        assert!(
            err.contains("this system has no GPIO character devices (/dev/gpiochip*)"),
            "{err}"
        );
    }

    #[test]
    fn a_line_past_the_end_names_the_range() {
        let chips = pi4_chips();
        assert!(check_line_offset(&chips[0], 57).is_ok());
        let err = check_line_offset(&chips[0], 58).unwrap_err().to_string();
        assert_eq!(err, "pinctrl-bcm2711 has 58 lines (0 to 57)");
        let err = check_line_offset(&chip(2, "", 0), 0)
            .unwrap_err()
            .to_string();
        assert_eq!(err, "gpiochip2 has no lines");
    }

    #[test]
    fn sysfs_number_maps_to_a_label_and_offset() {
        let chips = vec![
            (512, 58, "pinctrl-bcm2711".to_string()),
            (570, 8, "raspberrypi-exp-gpio".to_string()),
        ];
        assert_eq!(
            sysfs_number_to_chardev(529, &chips),
            Some(("pinctrl-bcm2711".to_string(), 17))
        );
        assert_eq!(
            sysfs_number_to_chardev(571, &chips),
            Some(("raspberrypi-exp-gpio".to_string(), 1))
        );
        assert_eq!(sysfs_number_to_chardev(600, &chips), None);
        assert_eq!(sysfs_number_to_chardev(17, &chips), None);

        // A label two chips share would be refused by select_chip, so it is
        // not suggested.
        let shared = vec![
            (512, 8, "gpio-mockup".to_string()),
            (520, 8, "gpio-mockup".to_string()),
        ];
        assert_eq!(sysfs_number_to_chardev(513, &shared), None);
    }

    #[test]
    fn an_unreadable_chip_does_not_hide_a_readable_one() {
        let chips = pi4_chips();
        let unreadable = Some("failed to read GPIO chip /dev/gpiochip2: Permission denied");
        // The named chip was readable: selection succeeds.
        assert_eq!(
            select_readable_chip("pinctrl-bcm2711", &chips, unreadable)
                .unwrap()
                .name,
            "gpiochip0"
        );
        // No match: the unreadable chip is named, since it may be the one.
        let err = select_readable_chip("pinctrl-rp1", &chips, unreadable)
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("no GPIO chip matches \"pinctrl-rp1\""),
            "{err}"
        );
        assert!(err.ends_with("/dev/gpiochip2: Permission denied"), "{err}");
        // Nothing readable at all: the read failure is the answer.
        let err = select_readable_chip("pinctrl-bcm2711", &[], unreadable)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "failed to read GPIO chip /dev/gpiochip2: Permission denied"
        );
        // No unreadable chip: select_chip's own error.
        let err = select_readable_chip("pinctrl-bcm2711", &[], None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no GPIO character devices"), "{err}");
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

    /// Mock writer whose own `Drop` records a `Released` marker, standing in
    /// for `CdevGpio`'s line request being freed on drop.
    struct ReleasingGpio {
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl GpioWriter for ReleasingGpio {
        fn write_value(&mut self, high: bool) -> Result<()> {
            self.log
                .lock()
                .unwrap()
                .push(if high { "high" } else { "low" });
            Ok(())
        }
    }

    impl Drop for ReleasingGpio {
        fn drop(&mut self) {
            self.log.lock().unwrap().push("released");
        }
    }

    #[test]
    fn gpio_release_runs_before_the_line_request_is_freed() {
        for (inverted, expected) in [
            (false, ["high", "low", "released"]),
            (true, ["low", "high", "released"]),
        ] {
            let log = Arc::new(Mutex::new(Vec::new()));
            let mut ptt = GpioPtt::with_writer(17, inverted, ReleasingGpio { log: log.clone() });
            ptt.set_ptt(PttState::Tx).unwrap();
            drop(ptt);
            assert_eq!(*log.lock().unwrap(), expected, "inverted = {inverted}");
        }
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

    #[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
    #[test]
    fn open_chardev_on_a_missing_chip_names_the_selector() {
        let err = GpioPtt::open_chardev("coppa-test-no-such-chip", 3, false, "coppa-test")
            .err()
            .expect("no chip carries this label")
            .to_string();
        assert!(
            err.contains("no GPIO chip matches \"coppa-test-no-such-chip\""),
            "{err}"
        );
    }

    /// Needs a real GPIO line: `COPPA_TEST_GPIO_LINE=<chip>:<line>`, e.g.
    /// `pinctrl-bcm2711:17` on a Pi 4. Run with `--ignored`.
    #[cfg(all(target_os = "linux", feature = "gpio-ptt"))]
    #[test]
    #[ignore = "needs a real GPIO line in COPPA_TEST_GPIO_LINE=<chip>:<line>"]
    fn chardev_line_keys_and_releases_on_real_hardware() {
        use gpiocdev::line::Value;
        let spec = std::env::var("COPPA_TEST_GPIO_LINE").expect("set COPPA_TEST_GPIO_LINE");
        let (chip, line) = spec.rsplit_once(':').expect("<chip>:<line>");
        let line: u32 = line.parse().expect("a line number");
        let mut ptt = GpioPtt::open_chardev(chip, line, false, "coppa-test").unwrap();
        assert_eq!(ptt.writer.request.value(line).unwrap(), Value::Inactive);
        ptt.set_ptt(PttState::Tx).unwrap();
        assert_eq!(ptt.writer.request.value(line).unwrap(), Value::Active);
        ptt.set_ptt(PttState::Rx).unwrap();
        assert_eq!(ptt.writer.request.value(line).unwrap(), Value::Inactive);
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
