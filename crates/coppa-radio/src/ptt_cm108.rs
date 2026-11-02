//! CM108/CM119 HID GPIO PTT via raw Linux hidraw (`/dev/hidrawN`) writes.
//!
//! The C-Media CM108, CM119 and their clones are USB audio chips with a few
//! general-purpose I/O pins that the host drives through a HID output report.
//! Many USB ham radio interfaces wire one of those pins (GPIO 3 on most of
//! them) to the rig's PTT line: the AIOC (firmware 1.2.0 or later), the
//! Digirig Lite, the DRA series and other sound-card interfaces built on the
//! chip. The Digirig Mobile keys through a serial RTS line instead, so use
//! `serial:` for it.
//!
//! # Report layout
//!
//! One 5-byte write to the hidraw node sets the pins:
//! `[0, 0, iodata, iomask, 0]`. Byte 0 is the report number (0, because the
//! device has no numbered reports; see the kernel's hidraw documentation),
//! `iomask` makes the pin an output and `iodata` sets its level. GPIO `n`
//! (1 to 8) is bit `n - 1`. This is the layout Direwolf's `cm108.c` writes.
//!
//! The chip latches the level until the next report, so closing the device
//! never unkeys the rig: a keyed handle must write the RX report before it
//! goes away. `Cm108Ptt` does that on drop, and coppad's PTT guardian does it
//! after a `kill -9`.
//!
//! # Access
//!
//! Real device access ([`HidrawDevice`], `Cm108Ptt::open`) is Linux only, with
//! no new crate dependency: plain `std::fs` writes to the hidraw node. The
//! node is reopened for every report, as Direwolf does, so an interface that
//! re-enumerates at the same path (or the same udev symlink) keeps working.
//! The rest of `Cm108Ptt` is OS-agnostic and generic over [`HidWriter`], so it
//! is unit-tested with a recording mock on any platform.
//!
//! `/dev/hidraw*` nodes are root-only by default. The user running coppad
//! needs a udev rule that grants it write access, for example through the
//! `audio` group; see step 7 of `docs/tutorials/raspberry-pi-install.md`.

use crate::{PttControl, PttState};
use anyhow::Result;

/// The GPIO pin CM108-style interfaces wire to PTT unless stated otherwise
/// (also Direwolf's default).
pub const DEFAULT_GPIO: u8 = 3;

/// The output report that drives `gpio` (1 to 8) high or low:
/// `[0, 0, iodata, iomask, 0]`. Byte 0 is the report number (0: the device
/// has no numbered reports); `iomask` makes the pin an output, `iodata` sets
/// its level.
fn report(gpio: u8, high: bool) -> [u8; 5] {
    let bit = 1u8 << (gpio - 1);
    [0, 0, if high { bit } else { 0 }, bit, 0]
}

fn check_gpio(gpio: u8) -> Result<()> {
    if !(1..=8).contains(&gpio) {
        anyhow::bail!("CM108 GPIO {gpio} is out of range; expected 1 to 8");
    }
    Ok(())
}

/// Abstraction over "write one CM108 output report", so `Cm108Ptt` can be
/// tested against a recording mock on any OS.
pub trait HidWriter: Send {
    fn write_report(&mut self, report: &[u8; 5]) -> Result<()>;
}

/// A real `/dev/hidrawN` node (Linux only).
#[cfg(target_os = "linux")]
pub struct HidrawDevice {
    path: std::path::PathBuf,
}

#[cfg(target_os = "linux")]
impl HidrawDevice {
    /// Check that `path` opens for writing, without writing to it. Never
    /// creates it: a mistyped path is ENOENT, not a new file under /dev.
    pub fn open(path: impl Into<std::path::PathBuf>) -> Result<Self> {
        use anyhow::Context;
        let path = path.into();
        Self::open_for_write(&path)
            .with_context(|| format!("cannot open {} for writing", path.display()))?;
        Ok(Self { path })
    }

    fn open_for_write(path: &std::path::Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new().write(true).open(path)
    }
}

#[cfg(target_os = "linux")]
impl HidWriter for HidrawDevice {
    /// Opens the node for every report, as Direwolf does.
    fn write_report(&mut self, report: &[u8; 5]) -> Result<()> {
        use anyhow::Context;
        use std::io::Write;
        Self::open_for_write(&self.path)
            .and_then(|mut file| file.write_all(report))
            .with_context(|| format!("failed to write CM108 report to {}", self.path.display()))
    }
}

/// CM108 PTT: drives one GPIO pin of a CM108/CM119-style USB interface
/// high/low for TX/RX.
///
/// Generic over `W: HidWriter` so it can be driven by a real hidraw node
/// (`Cm108Ptt::open`, Linux only) or by a mock in tests
/// (`Cm108Ptt::with_writer`).
///
/// Releases PTT on drop if keyed.
pub struct Cm108Ptt<W: HidWriter> {
    device: String,
    gpio: u8,
    inverted: bool,
    state: PttState,
    writer: W,
}

impl<W: HidWriter> Cm108Ptt<W> {
    /// Construct from an already-opened writer (real or mock).
    ///
    /// * `device` - the hidraw path, kept for diagnostics/`device()` only.
    /// * `gpio` - the CM108 GPIO pin wired to PTT, 1 to 8; anything else is
    ///   an error.
    /// * `inverted` - If true, PTT=TX drives the pin LOW.
    pub fn with_writer(
        device: impl Into<String>,
        gpio: u8,
        inverted: bool,
        writer: W,
    ) -> Result<Self> {
        check_gpio(gpio)?;
        Ok(Self {
            device: device.into(),
            gpio,
            inverted,
            state: PttState::Rx,
            writer,
        })
    }

    /// The configured hidraw device path.
    pub fn device(&self) -> &str {
        &self.device
    }

    /// The configured GPIO pin (1 to 8).
    pub fn gpio(&self) -> u8 {
        self.gpio
    }

    /// Whether the logic is inverted.
    pub fn is_inverted(&self) -> bool {
        self.inverted
    }
}

#[cfg(target_os = "linux")]
impl Cm108Ptt<HidrawDevice> {
    /// Open a real hidraw node. Fails if `gpio` is outside 1 to 8, or if the
    /// node is missing or not writable; never creates it, and writes nothing.
    pub fn open(device: &str, gpio: u8, inverted: bool) -> Result<Self> {
        check_gpio(gpio)?;
        let writer = HidrawDevice::open(device)?;
        Self::with_writer(device, gpio, inverted, writer)
    }
}

impl<W: HidWriter> PttControl for Cm108Ptt<W> {
    fn set_ptt(&mut self, state: PttState) -> Result<()> {
        let active = matches!(state, PttState::Tx);
        let high = if self.inverted { !active } else { active };
        self.writer.write_report(&report(self.gpio, high))?;
        self.state = state;
        Ok(())
    }

    fn get_ptt(&mut self) -> Result<PttState> {
        Ok(self.state)
    }
}

impl<W: HidWriter> Drop for Cm108Ptt<W> {
    /// COP-31: write the RX report if this handle left the pin keyed
    /// (`state` only changes on a successful write). The chip latches, so
    /// without this the rig would stay keyed.
    fn drop(&mut self) {
        if self.state == PttState::Tx {
            if let Err(e) = PttControl::set_ptt(self, PttState::Rx) {
                tracing::error!(
                    device = %self.device,
                    gpio = self.gpio,
                    error = %e,
                    "Cm108Ptt: failed to release PTT on drop"
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

    const TX3: [u8; 5] = [0, 0, 4, 4, 0];
    const RX3: [u8; 5] = [0, 0, 0, 4, 0];

    /// Recording mock: keeps every successful report instead of touching a
    /// device. Clones share the log, so writes made while the owning
    /// `Cm108Ptt` drops stay observable.
    #[derive(Clone, Default)]
    struct MockHid {
        reports: Arc<Mutex<Vec<[u8; 5]>>>,
        /// When set, the next write fails (and is not recorded).
        fail_next_write: Arc<AtomicBool>,
    }

    impl HidWriter for MockHid {
        fn write_report(&mut self, report: &[u8; 5]) -> Result<()> {
            if self.fail_next_write.swap(false, Ordering::SeqCst) {
                anyhow::bail!("mock HID write failure");
            }
            self.reports.lock().unwrap().push(*report);
            Ok(())
        }
    }

    #[test]
    fn report_bytes_for_every_gpio() {
        for gpio in 1..=8u8 {
            let bit = 1u8 << (gpio - 1);
            let mock = MockHid::default();
            let mut ptt = Cm108Ptt::with_writer("/dev/hidraw0", gpio, false, mock.clone()).unwrap();
            ptt.set_ptt(PttState::Tx).unwrap();
            ptt.set_ptt(PttState::Rx).unwrap();
            assert_eq!(
                *mock.reports.lock().unwrap(),
                vec![[0, 0, bit, bit, 0], [0, 0, 0, bit, 0]],
                "gpio {gpio}"
            );
        }
        assert_eq!(report(3, true), TX3);
        assert_eq!(report(3, false), RX3);
    }

    #[test]
    fn cm108_ptt_keys_and_releases_gpio3() {
        let mock = MockHid::default();
        let mut ptt = Cm108Ptt::with_writer("/dev/hidraw0", 3, false, mock.clone()).unwrap();

        assert_eq!(ptt.get_ptt().unwrap(), PttState::Rx);
        ptt.set_ptt(PttState::Tx).unwrap();
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx);
        ptt.set_ptt(PttState::Rx).unwrap();

        assert_eq!(*mock.reports.lock().unwrap(), vec![TX3, RX3]);
    }

    #[test]
    fn inverted_cm108_ptt_drives_the_pin_low_on_tx() {
        let mock = MockHid::default();
        let mut ptt = Cm108Ptt::with_writer("/dev/hidraw0", 3, true, mock.clone()).unwrap();

        ptt.set_ptt(PttState::Tx).unwrap();
        ptt.set_ptt(PttState::Rx).unwrap();

        assert_eq!(
            *mock.reports.lock().unwrap(),
            vec![RX3, TX3],
            "inverted PTT should drive the pin LOW on TX and HIGH on RX"
        );
    }

    #[test]
    fn gpio_outside_1_to_8_is_an_error() {
        for gpio in [0u8, 9] {
            let err = Cm108Ptt::with_writer("/dev/hidraw0", gpio, false, MockHid::default())
                .err()
                .unwrap_or_else(|| panic!("gpio {gpio} should be rejected"));
            assert!(
                format!("{err:#}").contains("expected 1 to 8"),
                "gpio {gpio}: {err:#}"
            );
        }

        let ptt = Cm108Ptt::with_writer("/dev/hidraw2", 4, true, MockHid::default()).unwrap();
        assert_eq!(ptt.device(), "/dev/hidraw2");
        assert_eq!(ptt.gpio(), 4);
        assert!(ptt.is_inverted());
    }

    #[test]
    fn dropping_a_keyed_cm108_ptt_writes_the_rx_report() {
        for (inverted, expected) in [(false, [TX3, RX3]), (true, [RX3, TX3])] {
            let mock = MockHid::default();
            let mut ptt = Cm108Ptt::with_writer("/dev/hidraw0", 3, inverted, mock.clone()).unwrap();
            ptt.set_ptt(PttState::Tx).unwrap();
            drop(ptt);
            assert_eq!(
                *mock.reports.lock().unwrap(),
                expected,
                "inverted = {inverted}"
            );
        }
    }

    #[test]
    fn dropping_an_unkeyed_cm108_ptt_writes_nothing() {
        let mock = MockHid::default();
        drop(Cm108Ptt::with_writer("/dev/hidraw0", 3, false, mock.clone()).unwrap());
        assert!(mock.reports.lock().unwrap().is_empty());

        // Released explicitly before the drop: nothing more is written.
        let mock = MockHid::default();
        let mut ptt = Cm108Ptt::with_writer("/dev/hidraw0", 3, false, mock.clone()).unwrap();
        ptt.set_ptt(PttState::Tx).unwrap();
        ptt.set_ptt(PttState::Rx).unwrap();
        drop(ptt);
        assert_eq!(*mock.reports.lock().unwrap(), vec![TX3, RX3]);
    }

    #[test]
    fn a_failed_cm108_release_is_retried_on_drop() {
        let mock = MockHid::default();
        let mut ptt = Cm108Ptt::with_writer("/dev/hidraw0", 3, false, mock.clone()).unwrap();
        ptt.set_ptt(PttState::Tx).unwrap();
        mock.fail_next_write.store(true, Ordering::SeqCst);
        assert!(ptt.set_ptt(PttState::Rx).is_err());
        assert_eq!(ptt.get_ptt().unwrap(), PttState::Tx, "still keyed");
        drop(ptt);
        assert_eq!(*mock.reports.lock().unwrap(), vec![TX3, RX3]);
    }

    #[test]
    fn dropping_a_boxed_cm108_backend_releases_through_ptt_backend() {
        let mock = MockHid::default();
        let mut backend = PttBackend::Other(Box::new(
            Cm108Ptt::with_writer("/dev/hidraw0", 3, false, mock.clone()).unwrap(),
        ));
        backend.set_ptt(PttState::Tx).unwrap();
        drop(backend);
        assert_eq!(*mock.reports.lock().unwrap(), vec![TX3, RX3]);
    }

    /// A per-test scratch path under the system temp dir (not created).
    #[cfg(target_os = "linux")]
    fn scratch_path(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::AtomicUsize;
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("coppa-cm108-{}-{n}-{name}", std::process::id()))
    }

    /// A regular file standing in for `/dev/hidrawN`, pre-filled with junk.
    #[cfg(target_os = "linux")]
    fn stand_in(name: &str) -> std::path::PathBuf {
        let path = scratch_path(name);
        std::fs::write(&path, [0xff; 5]).unwrap();
        path
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hidraw_open_on_a_missing_path_fails_and_creates_nothing() {
        let path = scratch_path("no-such-hidraw");
        let err = Cm108Ptt::open(path.to_str().unwrap(), 3, false)
            .err()
            .expect("a missing hidraw node should fail to open");
        let text = format!("{err:#}");
        assert!(text.contains(path.to_str().unwrap()), "{text}");
        assert!(text.contains("(os error 2)"), "{text}");
        assert!(!path.exists(), "open must never create the node");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_regular_file_stand_in_holds_the_last_report() {
        let path = stand_in("latch");
        let mut ptt = Cm108Ptt::open(path.to_str().unwrap(), 3, false).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            [0xff; 5],
            "open writes nothing"
        );
        ptt.set_ptt(PttState::Tx).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), TX3);
        drop(ptt);
        assert_eq!(std::fs::read(&path).unwrap(), RX3);
        std::fs::remove_file(&path).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_second_handle_overwrites_the_latch() {
        // Models the daemon (first) and the PTT guardian (second).
        let path = stand_in("two-handles");
        let mut daemon = Cm108Ptt::open(path.to_str().unwrap(), 3, false).unwrap();
        let mut guardian = Cm108Ptt::open(path.to_str().unwrap(), 3, false).unwrap();
        daemon.set_ptt(PttState::Tx).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), TX3);
        guardian.set_ptt(PttState::Rx).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), RX3);
        drop(guardian);
        drop(daemon);
        assert_eq!(std::fs::read(&path).unwrap(), RX3);
        std::fs::remove_file(&path).unwrap();
    }
}
