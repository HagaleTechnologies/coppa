//! COP-31: out-of-process PTT guardian. `coppad` re-spawns itself as
//! `coppad --ptt-guardian <method> --guardian-rigctld-address <addr>` and
//! writes one byte per PTT transition to the guardian's stdin: `K` before
//! keying, `R` after a successful unkey. When stdin reaches EOF (the daemon
//! exited, however it died) and the last report was `K`, the guardian
//! releases PTT through its own rigctld connection or GPIO pin. Serial PTT is
//! excluded: `serialport` opens exclusively and Linux asserts DTR on open,
//! so a reopen would briefly key the rig it means to release.
//!
//! The daemon side ([`PttGuardian`]) is told about PTT only from
//! `EventLoop::set_ptt`, the one place the daemon drives its PTT backend; a
//! new direct `self.ptt.set_ptt` call elsewhere must report too.

use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use coppa_radio::{PttControl, PttState};

use crate::config::{PttConfig, RadioConfig};

/// Report byte: the daemon is about to key PTT.
pub const KEYED: u8 = b'K';
/// Report byte: the daemon has released PTT successfully.
pub const RELEASED: u8 = b'R';
/// How many times the guardian tries to release PTT before giving up.
pub const RELEASE_ATTEMPTS: u32 = 5;
/// Pause between release attempts.
pub const RELEASE_RETRY: Duration = Duration::from_millis(200);

/// How a guardian's watch ended.
#[derive(Debug)]
pub enum Outcome {
    /// The daemon exited with PTT released (or never keyed): nothing to do.
    ExitedUnkeyed,
    /// The daemon exited keyed and the guardian released PTT.
    Released,
    /// The daemon exited keyed and every release attempt failed.
    ReleaseFailed(anyhow::Error),
}

/// Guardian side: read reports until EOF or a read error (either means the
/// daemon is gone), then, if the last known state was keyed, call `release`
/// up to [`RELEASE_ATTEMPTS`] times, `retry` apart. Unknown bytes are
/// ignored. `release` is never called when the daemon exited unkeyed.
pub fn watch(
    mut input: impl Read,
    retry: Duration,
    mut release: impl FnMut() -> anyhow::Result<()>,
) -> Outcome {
    let mut keyed = false;
    let mut buf = [0u8; 64];
    loop {
        match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for &byte in &buf[..n] {
                    match byte {
                        KEYED => keyed = true,
                        RELEASED => keyed = false,
                        _ => {}
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if !keyed {
        return Outcome::ExitedUnkeyed;
    }
    let mut last_err = None;
    for attempt in 1..=RELEASE_ATTEMPTS {
        match release() {
            Ok(()) => return Outcome::Released,
            Err(e) => {
                tracing::debug!(attempt, error = %e, "PTT release attempt failed");
                last_err = Some(e);
                if attempt < RELEASE_ATTEMPTS {
                    std::thread::sleep(retry);
                }
            }
        }
    }
    Outcome::ReleaseFailed(last_err.unwrap_or_else(|| anyhow::anyhow!("no release attempted")))
}

/// Open a fresh backend for `ptt` and send RX.
///
/// Deliberately separate from `EventLoop::create_ptt`: that one falls back
/// to `NullPtt` when rigctld is unreachable, while the guardian must see
/// that as a failure and retry.
pub fn release_once(ptt: &PttConfig, rigctld_address: &str) -> anyhow::Result<()> {
    match ptt {
        PttConfig::Rigctld => {
            // Bound TCP establishment per resolved address, as reconnect does.
            // Hostname resolution itself still uses the system resolver.
            let mut client = coppa_radio::rigctld::RigctldClient::connect_timeout(
                rigctld_address,
                Duration::from_secs(2),
            )?;
            PttControl::set_ptt(&mut client, PttState::Rx)
        }
        PttConfig::Gpio { pin } => {
            #[cfg(all(feature = "gpio-ptt", target_os = "linux"))]
            {
                let pin_num: u32 = pin
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid GPIO pin {pin:?}"))?;
                let mut gpio = coppa_radio::ptt_gpio::GpioPtt::open(pin_num, false)?;
                gpio.set_ptt(PttState::Rx)
            }
            #[cfg(not(all(feature = "gpio-ptt", target_os = "linux")))]
            {
                anyhow::bail!(
                    "GPIO pin {pin}: PTT method 'gpio' requires coppad to be built with \
                     --features gpio-ptt, on Linux"
                )
            }
        }
        other => anyhow::bail!("the PTT guardian does not handle PTT method {other:?}"),
    }
}

/// Whether the daemon should run a guardian: `[radio] ptt_guardian` is set
/// and the backend is a connected rigctld client or a GPIO pin. A rigctld
/// that was unreachable at startup left `NullPtt`, which has nothing to
/// guard; serial is excluded (see the module docs).
pub fn ptt_guardian_eligible(radio: &RadioConfig, backend_is_rigctld: bool) -> bool {
    if !radio.ptt_guardian {
        return false;
    }
    match radio.ptt_config() {
        Ok(PttConfig::Rigctld) => backend_is_rigctld,
        Ok(PttConfig::Gpio { .. }) => true,
        _ => false,
    }
}

/// Daemon side of the guardian: the pipe to its stdin, and the child itself.
pub struct PttGuardian {
    link: Option<Box<dyn Write + Send>>,
    child: Option<Child>,
}

impl PttGuardian {
    /// Start `coppad --ptt-guardian …` for `radio`, in its own process group
    /// on Unix so a signal to the daemon's group does not reach it. Its
    /// stdout and stderr are inherited, so its log lines land with the
    /// daemon's.
    pub fn spawn(radio: &RadioConfig) -> io::Result<Self> {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("--ptt-guardian")
            .arg(&radio.ptt_method)
            .arg("--guardian-rigctld-address")
            .arg(&radio.rigctld_address)
            .stdin(Stdio::piped());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("guardian stdin was not piped"))?;
        Ok(Self {
            link: Some(Box::new(stdin)),
            child: Some(child),
        })
    }

    /// A guardian reporting into `writer`, with no child process.
    #[cfg(test)]
    pub fn from_writer(writer: Box<dyn Write + Send>) -> Self {
        Self {
            link: Some(writer),
            child: None,
        }
    }

    /// The guardian's process id, if it is a real child process.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Report a PTT transition: `K` when `keyed`, `R` otherwise. A failed
    /// write means the guardian is gone; it is logged once and reporting
    /// stops (no respawn).
    pub fn report(&mut self, keyed: bool) {
        let Some(link) = self.link.as_mut() else {
            return;
        };
        let byte = if keyed { KEYED } else { RELEASED };
        if let Err(e) = link.write_all(&[byte]).and_then(|()| link.flush()) {
            tracing::error!(
                error = %e,
                "PTT guardian is no longer running; if coppad is killed while transmitting, \
                 PTT may stay keyed"
            );
            self.link = None;
            if let Some(child) = self.child.as_mut() {
                let _ = child.try_wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn run(input: &[u8], fail_first: u32) -> (Outcome, u32) {
        let mut calls = 0;
        let outcome = watch(input, Duration::ZERO, || {
            calls += 1;
            if calls <= fail_first {
                anyhow::bail!("not yet")
            }
            Ok(())
        });
        (outcome, calls)
    }

    #[test]
    fn eof_after_keyed_releases_once() {
        let (outcome, calls) = run(b"K", 0);
        assert!(matches!(outcome, Outcome::Released));
        assert_eq!(calls, 1);
    }

    #[test]
    fn repeated_keyed_reports_release_once() {
        let (outcome, calls) = run(b"KKK", 0);
        assert!(matches!(outcome, Outcome::Released));
        assert_eq!(calls, 1);
    }

    #[test]
    fn eof_after_released_does_nothing() {
        let (outcome, calls) = run(b"KR", 0);
        assert!(matches!(outcome, Outcome::ExitedUnkeyed));
        assert_eq!(calls, 0);
    }

    #[test]
    fn eof_with_no_reports_does_nothing() {
        let (outcome, calls) = run(b"", 0);
        assert!(matches!(outcome, Outcome::ExitedUnkeyed));
        assert_eq!(calls, 0);
    }

    #[test]
    fn unknown_bytes_are_ignored() {
        let (outcome, calls) = run(b"KxR", 0);
        assert!(matches!(outcome, Outcome::ExitedUnkeyed));
        assert_eq!(calls, 0);
        let (outcome, calls) = run(b"RxK", 0);
        assert!(matches!(outcome, Outcome::Released));
        assert_eq!(calls, 1);
    }

    /// Yields `K`, then a read error.
    struct KeyedThenError {
        sent: bool,
    }

    impl Read for KeyedThenError {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.sent {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"));
            }
            self.sent = true;
            buf[0] = KEYED;
            Ok(1)
        }
    }

    #[test]
    fn a_read_error_counts_as_the_daemon_being_gone() {
        let mut calls = 0;
        let outcome = watch(KeyedThenError { sent: false }, Duration::ZERO, || {
            calls += 1;
            Ok(())
        });
        assert!(matches!(outcome, Outcome::Released));
        assert_eq!(calls, 1);
    }

    #[test]
    fn release_is_retried_until_it_succeeds() {
        let (outcome, calls) = run(b"K", 2);
        assert!(matches!(outcome, Outcome::Released));
        assert_eq!(calls, 3);
    }

    #[test]
    fn release_gives_up_after_bounded_attempts() {
        let (outcome, calls) = run(b"K", u32::MAX);
        assert!(matches!(outcome, Outcome::ReleaseFailed(_)));
        assert_eq!(calls, RELEASE_ATTEMPTS);
    }

    /// A writer appending into a shared buffer, optionally failing.
    #[derive(Clone, Default)]
    struct Shared {
        bytes: Arc<Mutex<Vec<u8>>>,
        fail: Arc<Mutex<bool>>,
    }

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if *self.fail.lock().unwrap() {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"));
            }
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn reports_are_single_bytes() {
        let shared = Shared::default();
        let mut guardian = PttGuardian::from_writer(Box::new(shared.clone()));
        assert_eq!(guardian.pid(), None);
        guardian.report(true);
        assert_eq!(*shared.bytes.lock().unwrap(), b"K");
        guardian.report(false);
        assert_eq!(*shared.bytes.lock().unwrap(), b"KR");
    }

    #[test]
    fn a_broken_link_stops_reporting_without_panicking() {
        let shared = Shared::default();
        let mut guardian = PttGuardian::from_writer(Box::new(shared.clone()));
        *shared.fail.lock().unwrap() = true;
        guardian.report(true);
        assert!(guardian.link.is_none(), "reporting stops after a failure");
        *shared.fail.lock().unwrap() = false;
        guardian.report(false);
        assert!(shared.bytes.lock().unwrap().is_empty());
    }

    #[test]
    fn eligibility_matrix() {
        let radio = |method: &str, guardian: bool| RadioConfig {
            ptt_method: method.to_string(),
            ptt_guardian: guardian,
            ..RadioConfig::default()
        };
        assert!(ptt_guardian_eligible(&radio("rigctld", true), true));
        assert!(
            !ptt_guardian_eligible(&radio("rigctld", true), false),
            "an unreachable rigctld fell back to NullPtt: nothing to guard"
        );
        assert!(ptt_guardian_eligible(&radio("gpio:17", true), false));
        for method in ["none", "vox", "serial:/dev/ttyUSB0:dtr"] {
            assert!(
                !ptt_guardian_eligible(&radio(method, true), false),
                "{method}"
            );
        }
        assert!(!ptt_guardian_eligible(&radio("rigctld", false), true));
        assert!(!ptt_guardian_eligible(&radio("gpio:17", false), false));
    }

    #[test]
    fn release_once_refuses_methods_it_does_not_guard() {
        assert!(release_once(&PttConfig::None, "127.0.0.1:1").is_err());
        let serial = PttConfig::parse("serial:/dev/ttyUSB0:dtr").unwrap();
        assert!(release_once(&serial, "127.0.0.1:1").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rigctld_release_bounds_connection_establishment() {
        use std::net::{TcpListener, TcpStream};
        use std::sync::mpsc;

        // A full Linux accept queue drops new SYNs, exercising a stalled
        // connect without relying on an external host or firewall rules.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut pending = Vec::new();
        let mut saturated = false;
        for _ in 0..1024 {
            match TcpStream::connect_timeout(&addr, Duration::from_millis(50)) {
                Ok(stream) => pending.push(stream),
                Err(e) => {
                    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
                    saturated = true;
                    break;
                }
            }
        }
        assert!(saturated, "failed to fill the listener's accept queue");

        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(release_once(&PttConfig::Rigctld, &addr.to_string()))
                .unwrap();
        });
        let result = rx.recv_timeout(Duration::from_secs(3));
        // Unblock even the old, unbounded connect before joining the worker.
        drop(listener);
        drop(pending);
        worker.join().unwrap();
        assert!(
            result
                .expect("guardian connect exceeded its two-second timeout")
                .is_err(),
            "a saturated listener cannot accept the release connection"
        );
    }
}
