//! COP-31: drive the real `coppad` binary against a recording fake rigctld
//! and check that PTT is released on each way the daemon can stop.
//!
//! Every test keys PTT the way an operator would: it sends `TUNE 20` on the
//! VARA command port, which uses the same `transmit_samples` path as real
//! traffic, then stops the daemon while the 20 s tone is still playing.
//!
//! Each test owns its fake rigctld, its VARA ports (picked by the OS, never
//! fixed) and its temporary directory, so the tests run in parallel.
//!
//! Unix only: the tests send real signals and use process groups. They are
//! also skipped with `cpal-backend`: that build opens the host's default
//! audio device, and on a machine without one the transmission ends at once,
//! leaving no "mid-transmission" moment to stop the daemon in (the same
//! reason `tests/cli.rs` skips its running-daemon tests there).
// COP-142: the std-clock ban in clippy.toml targets event_loop only.
#![allow(clippy::disallowed_methods)]
#![cfg(all(unix, not(feature = "cpal-backend")))]

use std::fs::File;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

mod support;
use support::{
    free_ports, process_alive, send_signal, stays_false, strip_ansi, wait_until, Event,
    FakeRigctld, TempDir,
};

/// How long the daemon may take to log `Daemon ready`. Generous: debug
/// builds of five daemons start in parallel.
const READY_DEADLINE: Duration = Duration::from_secs(30);
/// How long `TUNE` may take to key PTT (the tone is generated first).
const KEY_DEADLINE: Duration = Duration::from_secs(30);
/// How long the daemon may take to exit after SIGTERM.
const EXIT_DEADLINE: Duration = Duration::from_secs(5);
/// How long the guardian may take to release PTT after the daemon dies.
const GUARDIAN_DEADLINE: Duration = Duration::from_secs(5);

const GUARDIAN_STARTED: &str = "PTT guardian started pid=";

// ---------------------------------------------------------------------------
// The daemon under test
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    /// The test binary's process group (the default).
    Inherit,
    /// A new process group led by coppad itself.
    Own,
}

/// A running `coppad` with rigctld PTT and the VARA server on.
struct Coppad {
    child: Child,
    dir: TempDir,
    command_port: u16,
    /// Held open so the daemon keeps its VARA client for the whole test.
    vara: Option<TcpStream>,
    reaped: bool,
}

impl Coppad {
    fn start(fake: &FakeRigctld, ptt_guardian: bool, group: Group) -> Self {
        Self::start_with_pre_delay(fake, ptt_guardian, group, 0)
    }

    /// As [`Coppad::start`], with `[radio] ptt_pre_delay_ms` set. A long
    /// pre-delay holds the event loop between keying and the first sample,
    /// so a first stop request cannot finish while it lasts (COP-85).
    fn start_with_pre_delay(
        fake: &FakeRigctld,
        ptt_guardian: bool,
        group: Group,
        ptt_pre_delay_ms: u64,
    ) -> Self {
        let dir = TempDir::new("daemon");
        let (command_port, data_port) = free_ports();
        let config = format!(
            "[radio]\n\
             ptt_method = \"rigctld\"\n\
             rigctld_address = \"{}\"\n\
             ptt_pre_delay_ms = {ptt_pre_delay_ms}\n\
             ptt_guardian = {ptt_guardian}\n\
             \n\
             [host]\n\
             vara_enabled = true\n\
             vara_command_port = {command_port}\n\
             vara_data_port = {data_port}\n\
             \n\
             [engine]\n\
             # COP-37: no transmission without a valid callsign.\n\
             callsign = \"N0CALL\"\n",
            fake.address()
        );
        let config_path = dir.path().join("coppad.toml");
        std::fs::write(&config_path, config).expect("write coppad.toml");
        let stdout = File::create(dir.path().join("stdout")).expect("create stdout capture");
        let stderr = File::create(dir.path().join("stderr")).expect("create stderr capture");

        let mut command = Command::new(env!("CARGO_BIN_EXE_coppad"));
        command
            .arg("--config")
            .arg(&config_path)
            .current_dir(dir.path())
            .env_remove("RUST_LOG")
            .env_remove("RUST_BACKTRACE")
            .env_remove("RUST_LIB_BACKTRACE")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        if group == Group::Own {
            command.process_group(0);
        }
        let child = command.spawn().expect("spawn coppad");
        let mut daemon = Self {
            child,
            dir,
            command_port,
            vara: None,
            reaped: false,
        };
        daemon.wait_for_log("Daemon ready", READY_DEADLINE);
        daemon
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Everything coppad (and the guardian, which inherits its stdout and
    /// stderr) has printed so far, without colour codes.
    fn output(&self) -> String {
        let read =
            |name: &str| std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default();
        strip_ansi(&format!(
            "--- stdout ---\n{}\n--- stderr ---\n{}",
            read("stdout"),
            read("stderr")
        ))
    }

    fn wait_for_log(&mut self, needle: &str, timeout: Duration) {
        let start = Instant::now();
        loop {
            if self.output().contains(needle) {
                return;
            }
            if let Some(status) = self.try_exit() {
                panic!(
                    "coppad exited with {status} before logging {needle:?}\n{}",
                    self.output()
                );
            }
            if start.elapsed() >= timeout {
                panic!(
                    "coppad did not log {needle:?} within {timeout:?}\n{}",
                    self.output()
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The guardian's pid, from `PTT guardian started pid=<n>`.
    fn guardian_pid(&self) -> Option<u32> {
        let output = self.output();
        let rest = &output[output.find(GUARDIAN_STARTED)? + GUARDIAN_STARTED.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    /// Send `TUNE 20` on the VARA command port and wait for the rig to be
    /// keyed.
    fn key(&mut self, fake: &FakeRigctld) {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.command_port));
        let mut stream = None;
        wait_until(Duration::from_secs(5), || {
            stream = TcpStream::connect(addr).ok();
            stream.is_some()
        });
        let mut stream = stream.unwrap_or_else(|| {
            panic!(
                "could not connect to the VARA command port {addr}\n{}",
                self.output()
            )
        });
        stream.write_all(b"TUNE 20\r").expect("send TUNE");
        self.vara = Some(stream);
        assert!(
            wait_until(KEY_DEADLINE, || fake
                .ptt_commands(0)
                .contains(&"T 1".to_string())),
            "TUNE did not key PTT within {KEY_DEADLINE:?}; rigctld log {:?}\n{}",
            fake.log(),
            self.output()
        );
    }

    fn try_exit(&mut self) -> Option<ExitStatus> {
        let status = self.child.try_wait().expect("poll coppad");
        if status.is_some() {
            self.reaped = true;
        }
        status
    }

    fn wait_exit(&mut self, timeout: Duration) -> ExitStatus {
        let mut status = None;
        wait_until(timeout, || {
            status = self.try_exit();
            status.is_some()
        });
        status
            .unwrap_or_else(|| panic!("coppad did not exit within {timeout:?}\n{}", self.output()))
    }

    /// SIGKILL coppad alone and reap it.
    fn sigkill(&mut self) {
        self.child.kill().expect("SIGKILL coppad");
        self.child.wait().expect("reap coppad");
        self.reaped = true;
    }
}

impl Drop for Coppad {
    fn drop(&mut self) {
        let guardian = self.guardian_pid();
        if !self.reaped {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // A failing test must not leak the guardian either.
        if let Some(pid) = guardian {
            if process_alive(pid) {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn sigterm_mid_transmission_releases_ptt_before_exit() {
    let fake = FakeRigctld::start();
    let mut daemon = Coppad::start(&fake, true, Group::Inherit);
    daemon.key(&fake);

    send_signal("-TERM", &daemon.pid().to_string());
    let status = daemon.wait_exit(EXIT_DEADLINE);
    assert_eq!(status.code(), Some(0), "exit status\n{}", daemon.output());

    assert!(
        wait_until(Duration::from_secs(2), || fake.has_eof(0)),
        "the daemon's rigctld connection did not close; log {:?}",
        fake.log()
    );
    // Startup release, the key, the shutdown release.
    assert_eq!(
        fake.ptt_commands(0),
        ["T 0", "T 1", "T 0"],
        "rigctld log {:?}\n{}",
        fake.log(),
        daemon.output()
    );
    let events = fake.events(0);
    let last_release = events
        .iter()
        .rposition(|e| *e == Event::Cmd("T 0".to_string()))
        .unwrap();
    let eof = events.iter().position(|e| *e == Event::Eof).unwrap();
    assert!(
        last_release < eof,
        "the release must come before the connection closes: {events:?}"
    );
    assert_eq!(fake.ptt_at_eof(0), Some(0), "rigctld log {:?}", fake.log());
    assert!(
        daemon.output().contains("Releasing PTT before shutdown"),
        "{}",
        daemon.output()
    );
}

#[test]
fn sigkill_of_the_daemons_process_group_is_still_released() {
    let fake = FakeRigctld::start();
    // coppad leads its own group, so killing the group cannot hit this test.
    let mut daemon = Coppad::start(&fake, true, Group::Own);
    assert!(daemon.guardian_pid().is_some(), "{}", daemon.output());
    daemon.key(&fake);

    send_signal("-KILL", &format!("-{}", daemon.pid()));
    daemon.wait_exit(EXIT_DEADLINE);

    assert!(
        wait_until(GUARDIAN_DEADLINE, || fake.another_connection_released()),
        "the guardian did not survive the group SIGKILL and release PTT within \
         {GUARDIAN_DEADLINE:?}; log {:?}\n{}",
        fake.log(),
        daemon.output()
    );
    assert_eq!(fake.ptt(), 0, "rigctld log {:?}", fake.log());
}

#[test]
fn clean_sigterm_leaves_the_guardian_quiet() {
    let fake = FakeRigctld::start();
    let mut daemon = Coppad::start(&fake, true, Group::Inherit);
    let guardian = daemon
        .guardian_pid()
        .unwrap_or_else(|| panic!("no guardian pid logged\n{}", daemon.output()));
    assert!(process_alive(guardian), "guardian {guardian} not running");
    daemon.key(&fake);

    send_signal("-TERM", &daemon.pid().to_string());
    let status = daemon.wait_exit(EXIT_DEADLINE);
    assert_eq!(status.code(), Some(0), "exit status\n{}", daemon.output());

    assert!(
        stays_false(Duration::from_secs(1), || fake.connections() > 1),
        "the guardian connected to rigctld after a clean exit; log {:?}\n{}",
        fake.log(),
        daemon.output()
    );
    assert!(
        wait_until(Duration::from_secs(2), || !process_alive(guardian)),
        "guardian {guardian} still running 2 s after a clean exit\n{}",
        daemon.output()
    );
    assert_eq!(fake.ptt(), 0, "rigctld log {:?}", fake.log());
}

#[test]
fn guardian_disabled_by_config_leaves_sigkill_unreleased() {
    let fake = FakeRigctld::start();
    let mut daemon = Coppad::start(&fake, false, Group::Inherit);
    assert!(
        !daemon.output().contains("PTT guardian started"),
        "{}",
        daemon.output()
    );
    daemon.key(&fake);

    daemon.sigkill();

    assert!(
        stays_false(Duration::from_secs(2), || fake.connections() > 1),
        "something connected to rigctld with the guardian disabled; log {:?}\n{}",
        fake.log(),
        daemon.output()
    );
    // The documented residual: nothing in software releases this case.
    assert!(
        wait_until(Duration::from_secs(2), || fake.has_eof(0)),
        "log {:?}",
        fake.log()
    );
    assert_eq!(fake.ptt_at_eof(0), Some(1), "rigctld log {:?}", fake.log());
    assert_eq!(fake.ptt(), 1, "rigctld log {:?}", fake.log());
}

/// COP-85: a second stop request while the first waits behind a long
/// pre-delay forces the exit: the daemon releases PTT on its own rigctld
/// connection and exits 130 at once, instead of staying keyed until the
/// pre-delay ends.
fn second_stop_request_forces_the_exit(first: &str, first_name: &str) {
    let fake = FakeRigctld::start();
    let mut daemon = Coppad::start_with_pre_delay(&fake, true, Group::Inherit, 15_000);
    daemon.key(&fake);

    send_signal(first, &daemon.pid().to_string());
    daemon.wait_for_log(
        &format!("Received {first_name}, shutting down"),
        EXIT_DEADLINE,
    );
    assert!(
        stays_false(Duration::from_millis(300), || daemon.try_exit().is_some()),
        "the pre-delay must hold the first stop request\n{}",
        daemon.output()
    );

    let forced_at = Instant::now();
    send_signal("-INT", &daemon.pid().to_string());
    let status = daemon.wait_exit(EXIT_DEADLINE);
    assert_eq!(status.code(), Some(130), "exit status\n{}", daemon.output());
    assert!(
        forced_at.elapsed() < Duration::from_secs(5),
        "the forced exit took {:?}",
        forced_at.elapsed()
    );
    assert_eq!(
        fake.ptt_commands(0).last().map(String::as_str),
        Some("T 0"),
        "the daemon's own connection must release PTT; rigctld log {:?}\n{}",
        fake.log(),
        daemon.output()
    );
    let output = daemon.output();
    for line in [
        "Received SIGINT again; forcing exit",
        "Releasing PTT for a forced exit",
        "Daemon stopped before its shutdown finished",
    ] {
        assert!(output.contains(line), "missing {line:?}\n{output}");
    }
}

#[test]
fn a_second_sigint_during_a_long_pre_delay_releases_ptt_and_exits_130() {
    second_stop_request_forces_the_exit("-INT", "SIGINT");
}

#[test]
fn sigterm_then_sigint_also_forces_the_exit() {
    second_stop_request_forces_the_exit("-TERM", "SIGTERM");
}
