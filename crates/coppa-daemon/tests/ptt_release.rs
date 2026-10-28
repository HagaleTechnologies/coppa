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
#![cfg(all(unix, not(feature = "cpal-backend")))]

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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

/// Poll `pred` every 10 ms until it holds or `timeout` passes.
fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if pred() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Check that `pred` stays false for the whole of `period`.
fn stays_false(period: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < period {
        if pred() {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    !pred()
}

// ---------------------------------------------------------------------------
// Fake rigctld
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Connect,
    Cmd(String),
    Eof,
}

/// One logged event: which connection, what happened, and the rig's PTT
/// state (`0`/`1`) right after it.
#[derive(Debug, Clone)]
struct Record {
    conn: usize,
    event: Event,
    ptt: u8,
}

#[derive(Default)]
struct RigState {
    ptt: u8,
    connections: usize,
    log: Vec<Record>,
}

/// A rigctld stand-in that accepts any number of connections, answers the
/// commands `RigctldClient` sends, and records everything. PTT state is
/// shared across connections, as it is on a real rig.
struct FakeRigctld {
    addr: SocketAddr,
    state: Arc<Mutex<RigState>>,
    stop: Arc<AtomicBool>,
}

impl FakeRigctld {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake rigctld");
        listener
            .set_nonblocking(true)
            .expect("nonblocking fake rigctld listener");
        let addr = listener.local_addr().expect("fake rigctld address");
        let state = Arc::new(Mutex::new(RigState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let state = state.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // BSD/macOS accepted sockets inherit the
                            // listener's O_NONBLOCK; serve() needs blocking
                            // reads or WouldBlock reads as EOF.
                            stream
                                .set_nonblocking(false)
                                .expect("blocking fake rigctld stream");
                            let conn = {
                                let mut s = state.lock().unwrap();
                                let conn = s.connections;
                                s.connections += 1;
                                let ptt = s.ptt;
                                s.log.push(Record {
                                    conn,
                                    event: Event::Connect,
                                    ptt,
                                });
                                conn
                            };
                            let state = state.clone();
                            thread::spawn(move || serve(conn, stream, &state));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
        }
        Self { addr, state, stop }
    }

    fn address(&self) -> String {
        self.addr.to_string()
    }

    fn ptt(&self) -> u8 {
        self.state.lock().unwrap().ptt
    }

    fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }

    fn log(&self) -> Vec<Record> {
        self.state.lock().unwrap().log.clone()
    }

    fn events(&self, conn: usize) -> Vec<Event> {
        self.log()
            .into_iter()
            .filter(|r| r.conn == conn)
            .map(|r| r.event)
            .collect()
    }

    /// The PTT commands (`T 0`/`T 1`) connection `conn` sent, in order.
    fn ptt_commands(&self, conn: usize) -> Vec<String> {
        self.events(conn)
            .into_iter()
            .filter_map(|e| match e {
                Event::Cmd(c) if c.starts_with("T ") => Some(c),
                _ => None,
            })
            .collect()
    }

    /// Whether any connection other than the daemon's own (connection 0)
    /// has sent `T 0`.
    fn another_connection_released(&self) -> bool {
        self.log()
            .iter()
            .any(|r| r.conn > 0 && r.event == Event::Cmd("T 0".to_string()))
    }

    fn has_eof(&self, conn: usize) -> bool {
        self.events(conn).contains(&Event::Eof)
    }

    /// The rig's PTT state when connection `conn` closed.
    fn ptt_at_eof(&self, conn: usize) -> Option<u8> {
        self.log()
            .into_iter()
            .find(|r| r.conn == conn && r.event == Event::Eof)
            .map(|r| r.ptt)
    }
}

impl Drop for FakeRigctld {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(conn: usize, stream: TcpStream, state: &Mutex<RigState>) {
    let mut writer = stream.try_clone().expect("clone fake rigctld stream");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).unwrap_or(0);
        let mut s = state.lock().unwrap();
        if n == 0 {
            let ptt = s.ptt;
            s.log.push(Record {
                conn,
                event: Event::Eof,
                ptt,
            });
            return;
        }
        let cmd = line.trim().to_string();
        let reply = if cmd == "f" {
            "14074000\n".to_string()
        } else if cmd == "t" {
            format!("{}\n", s.ptt)
        } else if let Some(value) = cmd.strip_prefix("T ") {
            s.ptt = u8::from(value.trim() != "0");
            "RPRT 0\n".to_string()
        } else if cmd == "m" {
            "USB\n2400\n".to_string()
        } else {
            "RPRT 0\n".to_string()
        };
        let ptt = s.ptt;
        s.log.push(Record {
            conn,
            event: Event::Cmd(cmd),
            ptt,
        });
        drop(s);
        if writer.write_all(reply.as_bytes()).is_err() {
            // The peer is gone; the next read reports the EOF.
            continue;
        }
    }
}

// ---------------------------------------------------------------------------
// The daemon under test
// ---------------------------------------------------------------------------

/// A uniquely named directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "coppad-ptt-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two distinct free TCP ports on loopback, picked by the OS.
fn free_ports() -> (u16, u16) {
    let a = TcpListener::bind("127.0.0.1:0").expect("bind free port");
    let b = TcpListener::bind("127.0.0.1:0").expect("bind free port");
    (
        a.local_addr().unwrap().port(),
        b.local_addr().unwrap().port(),
    )
}

/// Remove ANSI escape sequences (tracing's colour codes) from `text`.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Whether `pid` is a live process. A zombie (exited, not yet reaped by
/// its new parent) counts as gone.
fn process_alive(pid: u32) -> bool {
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state follows the parenthesised command name.
        return match stat.rfind(')') {
            Some(i) => !stat[i + 1..].trim_start().starts_with('Z'),
            None => true,
        };
    }
    if Path::new("/proc/self/stat").exists() {
        // procfs is mounted and has no entry for `pid`.
        return false;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn send_signal(signal: &str, target: &str) {
    let status = Command::new("kill")
        .args([signal, "--", target])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill {signal} -- {target} failed");
}

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
        let dir = TempDir::new("daemon");
        let (command_port, data_port) = free_ports();
        let config = format!(
            "[radio]\n\
             ptt_method = \"rigctld\"\n\
             rigctld_address = \"{}\"\n\
             ptt_pre_delay_ms = 0\n\
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
fn sigkill_mid_transmission_is_released_by_the_guardian() {
    let fake = FakeRigctld::start();
    let mut daemon = Coppad::start(&fake, true, Group::Inherit);
    assert!(daemon.guardian_pid().is_some(), "{}", daemon.output());
    daemon.key(&fake);

    daemon.sigkill();

    assert!(
        wait_until(GUARDIAN_DEADLINE, || fake.another_connection_released()),
        "no second rigctld connection sent T 0 within {GUARDIAN_DEADLINE:?}; log {:?}\n{}",
        fake.log(),
        daemon.output()
    );
    assert_eq!(fake.ptt(), 0, "rigctld log {:?}", fake.log());
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
